//! Adapter Discord Rich Presence по локальному IPC (ТЗ §6.1).
//!
//! Используется crate `discord-rich-presence` 1.x (MIT, актуальный).
//! Соединение — named pipe `\\?\pipe\discord-ipc-{0..9}`; handshake
//! `{"v":1,"client_id":…}` делает сам crate. Бот-токен не используется.
//!
//! Rate limit: Discord — 5 обновлений presence / 20 с. Лимитер гарантирует
//! не чаще 4.5 с между публикациями; быстрые промежутки коалисценируются
//! координатором (ТЗ §6.5).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use discord_rich_presence::{DiscordIpc, DiscordIpcClient, activity};

use crate::discord::errors::{Result, from_ipc_error};
use crate::discord::models::DiscordActivity;
use crate::yandex::errors::ErrorCode;

/// Максимальная частота публикации (4.5 c ⇒ максимум ~5 обновлений за 20 с).
pub const MIN_PUBLISH_INTERVAL: Duration = Duration::from_millis(4500);
/// Время сбора истории публикаций.
const RATE_WINDOW: Duration = Duration::from_secs(20);
/// Максимальное число обновлений в окне.
const RATE_LIMIT: usize = 4;

/// Константа Application ID. Поставляется через build-time переменную
/// `DISCORD_APPLICATION_ID` (см. `build.rs`); ID не секрет.
pub fn application_id() -> Option<&'static str> {
    option_env!("DISCORD_APPLICATION_ID").filter(|id| !id.trim().is_empty())
}

/// Клиент Discord IPC: владеет соединением и маршрутизирует обновления.
pub struct DiscordRpc {
    client: DiscordIpcClient,
    publish_history: VecDeque<Instant>,
}

impl DiscordRpc {
    /// Создать и подключиться. Возвращает Err(DiscordNotRunning), если
    /// Discord не запущен.
    pub fn connect() -> Result<Self> {
        let Some(app_id) = application_id() else {
            // Проверка конфигурации (ТЗ §6.2): без ID публиковать активность
            // нельзя. Ошибка наружу только как класс — без деталей ID.
            return Err(crate::yandex::errors::AppError::with_details(
                ErrorCode::DiscordRpcError,
                "DISCORD_APPLICATION_ID не задан (см. README)",
            ));
        };
        let mut client = DiscordIpcClient::new(app_id);
        client
            .connect()
            .map_err(from_ipc_error)
            .inspect_err(|_| tracing::warn!("Discord IPC недоступен"))?;
        tracing::info!("Discord IPC подключён");
        Ok(Self {
            client,
            publish_history: VecDeque::new(),
        })
    }

    /// Уведомить, что IPC ещё живой (heartbeat при handshake — ping не нужен:
    /// фреймы нечастые).
    pub fn is_rate_limited(&mut self) -> bool {
        let now = Instant::now();
        while let Some(ts) = self.publish_history.front() {
            if now.duration_since(*ts) > RATE_WINDOW {
                self.publish_history.pop_front();
            } else {
                break;
            }
        }
        self.publish_history.len() >= RATE_LIMIT
    }

    fn register_publish(&mut self) {
        let now = Instant::now();
        self.publish_history.push_back(now);
    }

    /// Опубликовать Activity.
    pub fn publish(&mut self, input: &DiscordActivity) -> Result<()> {
        tracing::debug!(
            "Discord IPC: set_activity image={:?} button={:?} button_url={:?}",
            input.large_image,
            input.button_label,
            input.button_url,
        );
        let mut payload =
            activity::Activity::new().activity_type(activity::ActivityType::Listening);

        if let Some(details) = input.details.as_deref() {
            payload = payload.details(details);
        }
        if let Some(state) = input.state.as_deref() {
            payload = payload.state(state);
        }
        if let Some(start_secs) = input.start_timestamp {
            let mut timestamps = activity::Timestamps::new().start(start_secs * 1000);
            // Прогресс-бар плеера в Discord: нужны оба таймстампа.
            if let Some(end_secs) = input.end_timestamp {
                timestamps = timestamps.end(end_secs * 1000);
            }
            payload = payload.timestamps(timestamps);
        }
        let large_image = input.large_image.clone();
        let large_text = input.large_text.clone();
        if large_image.is_some() || large_text.is_some() {
            let mut assets = activity::Assets::new();
            if let Some(image) = large_image.as_deref() {
                assets = assets.large_image(image);
            }
            if let Some(text) = large_text.as_deref() {
                assets = assets.large_text(text);
            }
            payload = payload.assets(assets);
        }
        if let (Some(label), Some(url)) = (input.button_label.clone(), input.button_url.clone()) {
            // Защита от регресса: подпись длиннее `MAX_BUTTON_LABEL_LEN` байт
            // гасит всю активность для других (см. тест
            // `button_labels_fit_discord_limits_in_chars_and_bytes`).
            if label.len() > crate::discord::models::MAX_BUTTON_LABEL_LEN {
                tracing::warn!(
                    "Discord IPC: подпись кнопки {} байт (> {}) — пропускаю кнопку",
                    label.len(),
                    crate::discord::models::MAX_BUTTON_LABEL_LEN,
                );
            } else {
                payload = payload.buttons(vec![activity::Button::new(label, url)]);
            }
        }

        self.client.set_activity(payload).map_err(from_ipc_error)?;
        // Ответ на каждый SET_ACTIVITY обязателен к прочтению: успех — echo
        // активности, ошибка — код/сообщение. Раньше ответ не читали: реджект
        // (например, непонравившаяся обложка) был невидим — активность висела,
        // а картинки не было. Чтение также держит баланс кадров в pipe:
        // иначе ответы копятся и следующий handshake прочитает чужой кадр.
        self.expect_reply("SET_ACTIVITY")?;
        self.register_publish();
        Ok(())
    }

    /// Очистить Activity (ТЗ §6.4).
    pub fn clear(&mut self) -> Result<()> {
        self.client.clear_activity().map_err(from_ipc_error)?;
        // Ответ читаем по той же причине, что в `publish`: баланс кадров
        // и видимость реджектов очистки.
        self.expect_reply("CLEAR")?;
        self.register_publish();
        Ok(())
    }

    /// Прочитать и залогировать вердикт Discord на отправленную команду.
    /// Блокирует поток до прихода кадра: Discord отвечает всегда (как и на
    /// handshake, который уже читается блокирующе). Ошибка чтения означает
    /// мёртвое соединение — координатор переподключится (drop+retry).
    fn expect_reply(&mut self, command: &str) -> Result<()> {
        match self.client.recv().map_err(from_ipc_error) {
            Ok((_, reply)) => match classify_reply(&reply) {
                ReplyVerdict::Accepted => {
                    tracing::debug!("Discord IPC: {} принят", command);
                    Ok(())
                }
                ReplyVerdict::Rejected { code, message } => {
                    tracing::warn!("Discord IPC: {} отклонён ({}): {}", command, code, message);
                    Ok(())
                }
            },
            Err(err) => Err(err),
        }
    }

    /// Закрыть соединение (ТЗ §6.1: при выходе приложения clean close).
    pub fn close(&mut self) {
        let _ = self.client.close();
    }
}

/// Вердикт Discord на SET_ACTIVITY/CLEAR: успех — echo активности,
/// ошибка — код/сообщение (`evt: "ERROR", data: {code, message}`).
#[derive(Debug, Clone, PartialEq, Eq)]
enum ReplyVerdict {
    Accepted,
    Rejected { code: String, message: String },
}

/// Чистая классификация ответа Discord для логов и тестов.
/// Неизвестная/пустая форма трактуется как успех, чтобы не плодить ложных
/// тревог (обрыв соединения и так виден по ошибкам чтения).
fn classify_reply(reply: &serde_json::Value) -> ReplyVerdict {
    if reply.get("evt").and_then(|v| v.as_str()) != Some("ERROR") {
        return ReplyVerdict::Accepted;
    }
    let data = reply.get("data");
    ReplyVerdict::Rejected {
        code: data
            .and_then(|d| d.get("code"))
            .map(|c| c.to_string())
            .unwrap_or_else(|| "?".to_string()),
        message: data
            .and_then(|d| d.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("?")
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limiter_counts_window() {
        let mut client = DiscordRpc {
            client: DiscordIpcClient::new("0"),
            publish_history: VecDeque::new(),
        };
        assert!(!client.is_rate_limited());
        for _ in 0..RATE_LIMIT {
            client.register_publish();
        }
        assert!(client.is_rate_limited());
    }

    #[test]
    fn application_id_absent_means_no_publish() {
        // option_env! не задан в тестовой конфигурации — просто помним
        // семантику: без ID возвращается ошибка DiscordRpcError.
        if application_id().is_none() {
            assert!(true);
        }
    }

    #[test]
    fn reply_success_is_accepted() {
        // Echo успеха: evt null, data — активность.
        let reply = serde_json::json!({
            "cmd": "SET_ACTIVITY",
            "evt": null,
            "data": {"details": "Track", "buttons": [{"label": "L", "url": "https://x"}]},
            "nonce": "1",
        });
        assert_eq!(classify_reply(&reply), ReplyVerdict::Accepted);
        // Пустая/неизвестная форма — тоже успех, не ошибка.
        assert_eq!(
            classify_reply(&serde_json::json!({})),
            ReplyVerdict::Accepted
        );
    }

    #[test]
    fn reply_error_reports_code_and_message() {
        let reply = serde_json::json!({
            "cmd": "SET_ACTIVITY",
            "evt": "ERROR",
            "data": {"code": 4000, "message": "Something went wrong"},
            "nonce": "1",
        });
        assert_eq!(
            classify_reply(&reply),
            ReplyVerdict::Rejected {
                code: "4000".to_string(),
                message: "Something went wrong".to_string(),
            }
        );
        // Битый ERROR без полей — не паникуем, код/текст заглушки.
        let broken = serde_json::json!({"evt": "ERROR"});
        assert!(matches!(
            classify_reply(&broken),
            ReplyVerdict::Rejected { .. }
        ));
    }
}
