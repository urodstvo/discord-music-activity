//! Единая точка координации Presence (ТЗ §7).
//!
//! Координатор живёт в собственном std-потоке: sync API crate
//! `discord-rich-presence` хорошо ложится на блокирующий поток, а события
//! Ynison разбираются неблокирующим poll через `try_recv`.
//!
//! Отвечает за: debounce быстрых изменений (ТЗ §6.5), rate limit
//! (не чаще 4.5 с между публикациями ⇒ ≤5/20 с), grace period (§6.6),
//! очистку на Stopped/настройке паузы/выключенном Presence (§6.4) и
//! retry подключения Discord с backoff (§6.1).

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use tauri::AppHandle;
use tokio::sync::mpsc;

use crate::app_state::{AppSharedState, DiscordStatus};
use crate::config::model::AppSettings;
use crate::yandex::errors::ErrorCode;
use crate::discord::client::{DiscordRpc, MIN_PUBLISH_INTERVAL};
use crate::discord::models::DiscordActivity;
use crate::presence::backoff::Backoff;
use crate::presence::grace::GracePolicy;
use crate::presence::mapper::map_presence;
use crate::runtime::CoordinatorEvent;
use crate::yandex::models::{PlaybackState, PlaybackStatus};

/// Debounce быстрых последовательных изменений (ТЗ §6.5: 250–500 мс).
pub const DEBOUNCE: Duration = Duration::from_millis(300);
/// Частота микротика координатора (ленивый poll, без busy loop).
const TICK: Duration = Duration::from_millis(50);

/// Действие, ожидающее исполнения по дедлайну.
enum Pending {
    Publish(DiscordActivity),
    Clear,
}

/// Полное состояние координатора.
struct Coordinator {
    last_settings: AppSettings,
    current: Option<PlaybackState>,
    /// Момент последнего валидного Ynison-состояния (grace семантика §6.6).
    last_valid_at: Option<Instant>,
    published: Option<DiscordActivity>,
    pending: Option<Pending>,
    pending_deadline: Option<Instant>,
    last_publish: Instant,
    discord: Option<DiscordRpc>,
    discord_retry_at: Instant,
    discord_backoff: Backoff,
}

/// Запустить координатор. Блокирует вызывающий поток до shutdown.
pub fn run_coordinator(
    shared: Arc<AppSharedState>,
    app: AppHandle,
    mut rx: mpsc::UnboundedReceiver<CoordinatorEvent>,
    shutdown: tokio_util::sync::CancellationToken,
) {
    let mut state = Coordinator {
        last_settings: shared.settings(),
        current: None,
        last_valid_at: None,
        published: None,
        pending: None,
        pending_deadline: None,
        last_publish: Instant::now() - Duration::from_secs(60),
        discord: None,
        discord_retry_at: Instant::now(),
        discord_backoff: Backoff::new(),
    };

    loop {
        if shutdown.is_cancelled() {
            break;
        }

        // 1. Неблокирующий разбор событий Ynison.
        while let Ok(event) = rx.try_recv() {
            state.consume(&event);
        }

        // 2. Настройки: смена требует пересмотр желаемого Activity.
        let fresh = shared.settings();
        let settings_changed = fresh != state.last_settings;
        state.last_settings = fresh.clone();

        let now = Instant::now();

        // 3. Спланировать publish/clear.
        state.schedule(&shared, now, settings_changed, &app);

        // 4. Исполнить pending по дедлайну.
        let deadline = state.pending_deadline;
        if let Some(deadline) = deadline {
            if deadline <= now {
                state.execute(&shared, &app, now);
            }
        } else if state.discord.is_none() && state.discord_retry_at <= now {
            // Discord не подключён — предпринять тихое подключение, чтобы
            // Presence появился при последующем запуске Discord (ТЗ §6.1).
            state.try_connect(&shared, &app, now);
        }

        std::thread::sleep(TICK);
    }

    // ТЗ §6.1: при выходе приложения попытаться очистить Activity.
    if let Some(discord) = state.discord.as_mut() {
        let _ = discord.clear();
        discord.close();
    }
    shared.set_discord_status(DiscordStatus::Disconnected, &app);
}

impl Coordinator {
    fn consume(&mut self, event: &CoordinatorEvent) {
        match event {
            CoordinatorEvent::PlaybackState(next) => {
                if next.is_some() {
                    // Валидное состояние обновляет freshness (grace §6.6).
                    self.last_valid_at = Some(Instant::now());
                }
                self.current = next.clone();
            }
            CoordinatorEvent::RequiresAuth => {
                // Токен отклонён: актуальное состояние появляется только
                // после повторной авторизации — сброс playback.
                self.current = None;
                self.last_valid_at = None;
            }
            CoordinatorEvent::Stopped => {
                // Не сбрасываем мгновенно; grace удержит Presence кратко
                // (§6.6). valid перестаёт обновляться.
            }
            CoordinatorEvent::DiscordReset => {
                // Пересоздать подключение (ТЗ §10.1): заставить retry сейчас.
                if self.discord.is_some() {
                    self.discord = None;
                }
                self.discord_retry_at = Instant::now();
            }
        }
    }

    /// Желаемое Activity в данный момент.
    fn desired_activity(&self, _shared: &AppSharedState, now: Instant) -> Option<DiscordActivity> {
        let settings = &self.last_settings;
        if !settings.presence_enabled {
            return None;
        }

        let grace = GracePolicy::standard();
        match self.current.as_ref() {
            Some(playback) => {
                match playback.status {
                    PlaybackStatus::Playing => map_presence(playback, settings, SystemTime::now()),
                    PlaybackStatus::Paused | PlaybackStatus::Unknown => {
                        map_presence(playback, settings, SystemTime::now())
                    }
                    PlaybackStatus::Stopped => None,
                }
            }
            None => {
                // Нет актуальногопіtablet трека от Ynison: grace держит последнюю
                // активность, затем пусто (ТЗ §6.6).
                if grace.is_stale(self.last_valid_at, now) {
                    None
                } else {
                    self.published.clone()
                }
            }
        }
    }

    /// Спланировать publish/clear, когда желаемое Activity отличается от
    /// уже опубликованного.
    fn schedule(
        &mut self,
        shared: &Arc<AppSharedState>,
        now: Instant,
        _settings_changed: bool,
        app: &AppHandle,
    ) {
        // Выключенный Presence: очистка сразу, следующий шаг — не публиковать.
        if !shared.settings().presence_enabled {
            if self.published.is_some() {
                self.published = None;
                self.pending = Some(Pending::Clear);
                self.pending_deadline = Some(now);
                self.execute(shared, app, now);
            }
            self.pending = None;
            self.pending_deadline = None;
            return;
        }

        let desired = self.desired_activity(shared, now);

        match (desired, self.published.clone()) {
            (None, None) => {
                self.pending = None;
                self.pending_deadline = None;
            }
            (Some(activity), Some(published)) if published == activity => {
                // Изменений нет — ничего не публикуем (ТЗ §6.5).
                self.pending = None;
                self.pending_deadline = None;
            }
            (Some(activity), _) => {
                let differs = match &self.pending {
                    Some(Pending::Publish(existing)) => existing != &activity,
                    _ => true,
                };
                if differs {
                    self.pending = Some(Pending::Publish(activity));
                    // Debounce (ТЗ §6.5).
                    self.pending_deadline = Some(now + DEBOUNCE);
                }
            }
            (None, Some(_)) => {
                // Явная остановка или истёкший grace (ТЗ §6.6): очистка
                // немедленно.
                self.pending = Some(Pending::Clear);
                self.pending_deadline = Some(now);
            }
        }
    }

    /// Исполнить само pending-действие.
    fn execute(&mut self, shared: &Arc<AppSharedState>, app: &AppHandle, now: Instant) {
        let Some(action) = self.pending.take() else {
            self.pending_deadline = None;
            return;
        };
        match action {
            Pending::Publish(activity) => {
                if !self.try_connect(shared, app, now) {
                    // Retry подключится сам за backoff; возвращаем pending.
                    self.pending = Some(Pending::Publish(activity));
                    self.pending_deadline = Some(self.discord_retry_at);
                    return;
                }
                let ready_at = self.last_publish + MIN_PUBLISH_INTERVAL;
                if now < ready_at {
                    // Rate limit (6.5 / Discord 5/20с): публикация откладывается.
                    self.pending = Some(Pending::Publish(activity));
                    self.pending_deadline = Some(ready_at);
                    return;
                }
                let Some(discord) = self.discord.as_mut() else {
                    return;
                };
                match discord.publish(&activity) {
                    Ok(()) => {
                        self.published = Some(activity);
                        self.last_publish = now;
                        self.pending_deadline = None;
                        // Диагностика Presence: что именно ушло в Discord
                        // (данные отображаемые, секретов нет).
                        tracing::info!(
                            "Presence опубликовано: details={:?} state={:?} timestamps={:?}",
                            self.published.as_ref().and_then(|a| a.details.clone()),
                            self.published.as_ref().and_then(|a| a.state.clone()),
                            self.published.as_ref().and_then(|a| a.start_timestamp)
                        );
                    }
                    Err(err) => {
                        tracing::warn!(
                            "publish failed: {}",
                            crate::discord::errors::classify(&err)
                        );
                        self.drop_connection(shared, app);
                        self.pending = Some(Pending::Publish(activity));
                        self.pending_deadline = Some(self.discord_retry_at);
                    }
                }
            }
            Pending::Clear => {
                self.pending_deadline = None;
                if self.published.is_none() {
                    return;
                }
                if let Some(discord) = self.discord.as_mut() {
                    if !discord.is_rate_limited() {
                        match discord.clear() {
                            Ok(()) => {
                                self.published = None;
                                self.last_publish = now;
                                tracing::info!("Activity очищена");
                            }
                            Err(err) => {
                                tracing::warn!(
                                    "clear failed: {}",
                                    crate::discord::errors::classify(&err)
                                );
                                self.drop_connection(shared, app);
                            }
                        }
                    }
                } else {
                    // Discord не был подключён: Activity уже не существует.
                    self.published = None;
                }
            }
        }
    }

    /// Подключиться к Discord (не чаще backoff).
    fn try_connect(&mut self, shared: &Arc<AppSharedState>, app: &AppHandle, now: Instant) -> bool {
        if self.discord.is_some() {
            return true;
        }
        if now < self.discord_retry_at {
            return false;
        }
        shared.set_discord_status(DiscordStatus::Connecting, app);
        match DiscordRpc::connect() {
            Ok(discord) => {
                self.discord = Some(discord);
                self.discord_backoff.reset();
                shared.set_discord_status(DiscordStatus::Connected, app);
                tracing::info!("Discord IPC подключён");
                true
            }
            Err(err) => {
                shared.set_discord_status(DiscordStatus::Disconnected, app);
                let delay = self.discord_backoff.next_delay();
                self.discord_retry_at = now + delay;
                // Причина (класс, без секретов) — иначе в логе болтается
                // «не подключается» без объяснения.
                match err.code {
                    ErrorCode::DiscordNotRunning => {
                        tracing::warn!(
                            "Discord IPC недоступен ({}), retry через {} мс",
                            err.code,
                            delay.as_millis()
                        );
                    }
                    ErrorCode::DiscordRpcError => {
                        tracing::warn!(
                            "Discord IPC: {:?} ({:?}), retry через {} мс",
                            err.code,
                            err.details,
                            delay.as_millis()
                        );
                    }
                    _ => {
                        tracing::warn!("Discord IPC: {:?}", err.details);
                    }
                }
                false
            }
        }
    }

    fn drop_connection(&mut self, shared: &Arc<AppSharedState>, app: &AppHandle) {
        if let Some(discord) = self.discord.as_mut() {
            discord.close();
        }
        self.discord = None;
        let delay = self.discord_backoff.next_delay();
        self.discord_retry_at = Instant::now() + delay;
        shared.set_discord_status(DiscordStatus::Disconnected, app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playback(track_id: &str, status: PlaybackStatus) -> PlaybackState {
        PlaybackState {
            status,
            track_id: Some(track_id.into()),
            title: Some("Title".into()),
            artists: vec!["Artist".into()],
            album: None,
            duration_ms: Some(200_000),
            position_ms: Some(10_000),
            cover_url: None,
            track_url: None,
            source_device: None,
            observed_at: SystemTime::now(),
        }
    }

    fn coordinator(settings: AppSettings) -> Coordinator {
        Coordinator {
            last_settings: settings,
            current: None,
            last_valid_at: None,
            published: None,
            pending: None,
            pending_deadline: None,
            last_publish: Instant::now(),
            discord: None,
            discord_retry_at: Instant::now(),
            discord_backoff: Backoff::new(),
        }
    }

    #[test]
    fn desired_none_when_presence_disabled() {
        let shared = AppSharedState::new(AppSettings::default());
        let mut state = coordinator(AppSettings {
            presence_enabled: false,
            ..AppSettings::default()
        });
        state.current = Some(playback("1", PlaybackStatus::Playing));
        assert!(state.desired_activity(&shared, Instant::now()).is_none());
    }

    #[test]
    fn desired_maps_playing() {
        let shared = AppSharedState::new(AppSettings::default());
        let mut state = coordinator(shared.settings());
        state.current = Some(playback("1", PlaybackStatus::Playing));
        let desired = state.desired_activity(&shared, Instant::now()).unwrap();
        assert_eq!(desired.details.as_deref(), Some("Title"));
        assert_eq!(desired.state.as_deref(), Some("Artist"));
    }

    #[test]
    fn stopped_clears_despite_published() {
        let shared = AppSharedState::new(AppSettings::default());
        let mut state = coordinator(shared.settings());
        state.current = Some(playback("1", PlaybackStatus::Stopped));
        state.published = Some(DiscordActivity::listening());
        assert!(state.desired_activity(&shared, Instant::now()).is_none());
    }

    #[test]
    fn grace_holds_recent_activity_then_clears() {
        let shared = AppSharedState::new(AppSettings::default());
        let published = DiscordActivity::listening();
        // Свежий Ynison — держим последнюю активность.
        let mut state = coordinator(shared.settings());
        state.published = Some(published.clone());
        state.last_valid_at = Some(Instant::now() - Duration::from_secs(5));
        assert_eq!(
            state.desired_activity(&shared, Instant::now()),
            Some(published.clone())
        );
        // Grace истёк — желаемое пусто → очистка.
        state.last_valid_at = Some(Instant::now() - Duration::from_secs(15));
        assert!(state.desired_activity(&shared, Instant::now()).is_none());
    }
}
