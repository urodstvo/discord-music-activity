//! Управление источниками данных о воспроизведении (PLAN п.2: единый
//! контракт `sources/` + fallback через Windows Media Control) и Discord
//! reset.
//!
//! Supervisor больше не привязан к Ynison: арбитр сводит фактическое
//! подмножество запущенных источников к желаемому по настройке
//! `sourceMode`:
//! - `auto` — Ynison приоритетен (если токен есть и принят), SMTC-fallback
//!   включается, когда токена нет / токен отклонён / Ynison не подключается
//!   дольше `FALLBACK_AFTER`;
//! - `ynison` — как раньше, только Ynison;
//! - `media_control` — всегда SMTC, токен не требуется.
//!
//! Источники шлют `SourceEvent` в общий канал; арбитр решает, чьи состояния
//! публиковать (UI + координатор), и выдаёт каждому источнику токен жизни
//! через `CancellationToken`. Новые источники добавляются реализацией той
//! же сигнатуры `spawn_*` и правил в арбитре, остальной код не меняется.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::app_state::{AppSharedState, DiscordStatus, YnisonStatus};
use crate::config::model::SourceMode;
use crate::sources::{PlaybackState, SourceEvent, SourceKind};
use crate::yandex::auth;

/// Сколько секунд Ynison может не подключаться, прежде чем арбитр включит
/// SMTC-fallback в режиме `auto`. Согласовано с grace-политикой координатора
/// (ТЗ §6.6): пока Ynison не признан недоступным, Presence держит grace.
pub const FALLBACK_AFTER: Duration = Duration::from_secs(10);
/// Период самоосмотра арбитра (смена статуса Ynison без новых событий).
const RECONCILE_TICK: Duration = Duration::from_secs(2);

/// События для координатора (ТЗ §7).
#[derive(Debug)]
pub enum CoordinatorEvent {
    /// Нормализованное состояние (None — остановка).
    PlaybackState(Option<PlaybackState>),
    /// Токен отклонён сервером.
    RequiresAuth,
    /// Активный источник остановлен.
    Stopped,
    /// Пользователь запросил пересоздание Discord IPC (ТЗ §10.1).
    DiscordReset,
}

/// Внутреннее состояние арбитра.
struct ArbiterState {
    ynison: Option<CancellationToken>,
    smtc: Option<CancellationToken>,
    /// Момент старта Ynison (для порога fallback).
    ynison_started: Option<Instant>,
    /// Токен отклонён сервером — до повторного сохранения Ynison не рейдим.
    auth_blocked: bool,
    /// Есть ли токен в хранилище (обновляется через публичный API).
    token_present: bool,
    /// Чьи состояния публикуем (None — следующий валидный источник примется).
    active: Option<SourceKind>,
}

impl ArbiterState {
    fn wants_ynison(&self, mode: SourceMode) -> bool {
        match mode {
            SourceMode::MediaControl => false,
            SourceMode::Ynison | SourceMode::Auto => self.token_present && !self.auth_blocked,
        }
    }

    /// Fallback SMTC нужен прямо сейчас.
    fn wants_smtc(&self, mode: SourceMode, ynison_status: YnisonStatus, now: Instant) -> bool {
        match mode {
            SourceMode::MediaControl => true,
            SourceMode::Ynison => false,
            SourceMode::Auto => {
                if !self.token_present || self.auth_blocked {
                    return true;
                }
                if self.ynison.is_some() {
                    // Ynison запущен: fallback держим, пока он не подключился
                    // отведённое время. Статус Connected — Ynison жив.
                    let waited = self
                        .ynison_started
                        .map(|at| now.saturating_duration_since(at) >= FALLBACK_AFTER)
                        .unwrap_or(false);
                    ynison_status != YnisonStatus::Connected && waited
                } else {
                    false
                }
            }
        }
    }
}

/// Общий арбитр между публичным API Supervisor и роутер-задачей.
struct Arbiter {
    shared: Arc<AppSharedState>,
    coordinator_tx: mpsc::UnboundedSender<CoordinatorEvent>,
    source_tx: mpsc::UnboundedSender<SourceEvent>,
    state: Mutex<ArbiterState>,
}

impl Arbiter {
    /// Синхронизация жизненного цикла источников с желаемым подмножеством.
    /// Никаких блокирующих COM-вызовов под мьютексом: только spawn/cancel.
    fn reconcile(&self, app: &tauri::AppHandle) {
        let mut state = self.state.lock().expect("арбитр доступен");
        let now = Instant::now();
        let mode = self.shared.settings().source_mode;
        let ynison_status = self.shared.snapshot().ynison;

        // --- Ynison ---
        let want_ynison = state.wants_ynison(mode);
        if !want_ynison && state.ynison.is_some() {
            if let Some(token) = state.ynison.take() {
                token.cancel();
                state.ynison_started = None;
                tracing::info!("Ynison остановлен арбитром");
            }
            self.release_active(SourceKind::Ynison, &mut state, app);
        }
        if want_ynison && state.ynison.is_none() {
            match auth::load_token() {
                Ok(Some(token)) => {
                    let shutdown = self.shared.shutdown.child_token();
                    crate::sources::ynison::spawn_ynison_source(
                        token,
                        Arc::clone(&self.shared),
                        app.clone(),
                        shutdown.clone(),
                        self.source_tx.clone(),
                    );
                    state.ynison = Some(shutdown);
                    state.ynison_started = Some(now);
                    tracing::info!("Ynison запущен арбитром");
                }
                Ok(None) => {
                    // Токен исчез между проверками (удалён извне) —
                    // больше не считаем его присутствующим.
                    state.token_present = false;
                }
                Err(err) => {
                    // Хранилище секретов недоступно: сообщаем и не рейдим.
                    state.token_present = false;
                    self.shared.push_error(&err, app);
                }
            }
        }

        // --- SMTC fallback ---
        let want_smtc = state.wants_smtc(mode, ynison_status, now);
        if !want_smtc && state.smtc.is_some() {
            if let Some(token) = state.smtc.take() {
                token.cancel();
                tracing::info!("SMTC источник остановлен арбитром");
            }
            self.release_active(SourceKind::MediaControl, &mut state, app);
        }
        if want_smtc && state.smtc.is_none() {
            let shutdown = self.shared.shutdown.child_token();
            crate::sources::smtc::spawn_smtc_source(
                shutdown.clone(),
                self.source_tx.clone(),
                Arc::clone(&self.shared),
            );
            state.smtc = Some(shutdown);
            // Важно: если раньше публиковал Ynison (сейчас недоступен),
            // его метка active заблокировала бы приём SMTC-состояний
            // (active==Ynison). Сбрасываем: первый валидный источник
            // подхватится.
            state.active = None;
            self.shared.set_active_source(None, app);
            tracing::info!("SMTC источник запущен арбитром");
        }
    }

    /// Активный источник ушёл — освободить публикацию.
    fn release_active(&self, kind: SourceKind, state: &mut ArbiterState, app: &tauri::AppHandle) {
        if state.active == Some(kind) {
            state.active = None;
            self.shared.set_active_source(None, app);
        }
    }

    /// Допустим ли источник к публикации в текущий момент.
    fn eligible_incoming(kind: SourceKind, state: &ArbiterState, mode: SourceMode) -> bool {
        let source_alive = match kind {
            SourceKind::Ynison => state.ynison.is_some(),
            SourceKind::MediaControl => state.smtc.is_some(),
        };
        if !source_alive {
            return false;
        }
        match state.active {
            None => true,
            Some(active) if active == kind => true,
            // Ynison в Auto-mode имеет приоритет: его состояние забирает
            // публикацию даже при живой SMTC-публикации.
            Some(SourceKind::MediaControl)
                if kind == SourceKind::Ynison && mode == SourceMode::Auto =>
            {
                true
            }
            Some(_) => false,
        }
    }

    /// Обработка события от источника. Возвращает true, если после
    /// обработки нужно пересчитать подмножество источников.
    fn on_source_event(&self, event: SourceEvent, app: &tauri::AppHandle) -> bool {
        let mut state = self.state.lock().expect("арбитр доступен");
        let mode = self.shared.settings().source_mode;
        match event {
            SourceEvent::State {
                kind,
                state: playback,
            } => {
                if !Self::eligible_incoming(kind, &state, mode) {
                    tracing::debug!("события {:?} подавлены арбитром", kind);
                    return false;
                }
                if kind == SourceKind::Ynison && state.smtc.is_some() && mode == SourceMode::Auto {
                    // Ynison вернулся с данными — гасим fallback.
                    if let Some(token) = state.smtc.take() {
                        token.cancel();
                        tracing::info!("SMTC fallback остановлен: Ynison вернулся");
                    }
                }
                if state.active != Some(kind) {
                    state.active = Some(kind);
                    self.shared.set_active_source(Some(kind), app);
                }
                self.shared.push_playback(playback.clone(), app);
                let _ = self
                    .coordinator_tx
                    .send(CoordinatorEvent::PlaybackState(playback));
                false
            }
            SourceEvent::RequiresAuth => {
                if state.ynison.is_none() {
                    return false;
                }
                // Токен отклонён: актуальное состояние появится только после
                // повторной авторизации (ТЗ §5.1; endless-ретраи запрещены).
                let _ = self.coordinator_tx.send(CoordinatorEvent::RequiresAuth);
                let _ = self
                    .coordinator_tx
                    .send(CoordinatorEvent::PlaybackState(None));
                if mode == SourceMode::Auto {
                    state.auth_blocked = true;
                    self.release_active(SourceKind::Ynison, &mut state, app);
                    state.ynison_started = None;
                    // Задача выходит сама после RequiresAuth — дескриптор
                    // не cancel'им, событие Stopped придёт штатно.
                    tracing::warn!("токен отклонён: Ynison уйдёт, включаю SMTC fallback");
                    return true;
                }
                false
            }
            SourceEvent::Stopped { kind } => {
                // Финальное событие задачи: отпускаем дескриптор.
                match kind {
                    SourceKind::Ynison => {
                        let _ = state.ynison.take();
                        state.ynison_started = None;
                    }
                    SourceKind::MediaControl => {
                        let _ = state.smtc.take();
                    }
                }
                let was_active = state.active == Some(kind);
                self.release_active(kind, &mut state, app);
                if was_active {
                    let _ = self.coordinator_tx.send(CoordinatorEvent::Stopped);
                }
                false
            }
        }
    }
}

/// Supervisor: публичное управление источниками.
#[derive(Clone)]
pub struct Supervisor {
    shared: Arc<AppSharedState>,
    arbiter: Arc<Arbiter>,
}

impl Supervisor {
    pub fn shared(&self) -> Arc<AppSharedState> {
        Arc::clone(&self.shared)
    }

    pub fn new(
        shared: Arc<AppSharedState>,
        app: tauri::AppHandle,
    ) -> (Self, mpsc::UnboundedReceiver<CoordinatorEvent>) {
        let (coordinator_tx, coordinator_rx) = mpsc::unbounded_channel();
        let (source_tx, source_rx) = mpsc::unbounded_channel::<SourceEvent>();
        let arbiter = Arc::new(Arbiter {
            shared: Arc::clone(&shared),
            coordinator_tx,
            source_tx,
            state: Mutex::new(ArbiterState {
                ynison: None,
                smtc: None,
                ynison_started: None,
                auth_blocked: false,
                token_present: false,
                active: None,
            }),
        });

        // Роутер: события источников + периодическая сверка арбитра.
        let router_arbiter = Arc::clone(&arbiter);
        tauri::async_runtime::spawn(async move {
            let mut rx = source_rx;
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(RECONCILE_TICK) => {
                        router_arbiter.reconcile(&app);
                    }
                    event = rx.recv() => {
                        match event {
                            Some(event) => {
                                if router_arbiter.on_source_event(event, &app) {
                                    router_arbiter.reconcile(&app);
                                }
                            }
                            None => break,
                        }
                    }
                }
            }
        });

        (Self { shared, arbiter }, coordinator_rx)
    }

    /// Записать факт наличия токена в хранилище и пересчитать источники
    /// (вызывается один раз при старте приложения).
    pub fn set_token_present(&self, present: bool, app: &tauri::AppHandle) {
        {
            let mut state = self.arbiter.state.lock().expect("арбитр доступен");
            state.token_present = present;
        }
        // Снапшот auth_required должен отражать факт наличия токена уже на
        // старте — иначе UI до первого статуса Ynison показывает карточку
        // «введите токен» (см. комментарий в app_state).
        self.shared.set_token_present(present);
        self.arbiter.reconcile(app);
    }

    /// Токен сохранён (и уже проверен сервером) — разблокировать Ynison.
    /// Снапшот обновляем сразу (как при старте): иначе UI до первого
    /// `Connected` продолжает показывать «нет токена», хотя токен уже есть.
    /// Если Ynison позже отклонит токен, статус `AuthenticationRequired`
    /// вернёт флаг обратно.
    pub fn on_token_saved(&self, app: &tauri::AppHandle) {
        {
            let mut state = self.arbiter.state.lock().expect("арбитр доступен");
            state.token_present = true;
            state.auth_blocked = false;
        }
        self.shared.set_token_present(true);
        self.shared.publish_snapshot(app);
        self.arbiter.reconcile(app);
    }

    /// Токен удалён — источники переключаются по режиму (fallback SMTC).
    pub fn on_token_removed(&self, app: &tauri::AppHandle) {
        {
            let mut state = self.arbiter.state.lock().expect("арбитр доступен");
            state.token_present = false;
            state.auth_blocked = false;
        }
        self.arbiter.reconcile(app);
    }

    /// Настройки изменились (например, `sourceMode`) — пересчитать
    /// подмножество источников без их обязательной перезапуска.
    pub fn settings_changed(&self, app: &tauri::AppHandle) {
        self.arbiter.reconcile(app);
    }

    /// Явная команда «Повторить подключение» (ТЗ §10.1).
    ///
    /// Если токен на месте, снимаем auth_blocked: пользователь явно выражает
    /// намерение ещё раз попробовать сеть. Источники пересчитываются.
    pub fn reconnect_yandex(&self, app: &tauri::AppHandle) {
        {
            let mut state = self.arbiter.state.lock().expect("арбитр доступен");
            if state.token_present {
                state.auth_blocked = false;
            }
        }
        self.arbiter.reconcile(app);
    }

    /// Пересоздать Discord IPC подключение (ТЗ §10.1).
    pub fn reconnect_discord(&self, app: &tauri::AppHandle) {
        self.shared
            .set_discord_status(DiscordStatus::Connecting, app);
        let _ = self
            .arbiter
            .coordinator_tx
            .send(CoordinatorEvent::DiscordReset);
    }

    /// Полная остановка всех фоновых задач (ТЗ §6.4: quit/logout).
    pub fn shutdown_all(&self) {
        // Глобальный токен гасит источники через child-токены и координатор.
        self.shared.shutdown.cancel();
    }
}
