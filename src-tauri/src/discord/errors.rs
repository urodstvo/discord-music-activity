//! Ошибки Discord IPC, приведённые к общему `AppError` (ТЗ §11.2).

use crate::yandex::errors::{AppError, AppResult, ErrorCode};

pub type Result<T> = AppResult<T>;

/// Сопоставить ошибку crate `discord-rich-presence` с классами приложения.
pub fn from_ipc_error(err: discord_rich_presence::error::Error) -> AppError {
    use discord_rich_presence::error::Error as E;
    match err {
        // Не нашли pipe / не подключились → «Discord не запущен» + retry.
        E::IPCNotFound | E::IPCConnectionFailed | E::NotConnected => {
            AppError::new(ErrorCode::DiscordNotRunning)
        }
        // Всё остальное (decor кодеков/чтения/записи) — RPC ошибка.
        _ => AppError::new(ErrorCode::DiscordRpcError),
    }
}

/// Текст ошибки не содержит секретов: Discord IPC не работает с ними.
pub fn classify(err: &AppError) -> &'static str {
    match err.code {
        ErrorCode::DiscordNotRunning => "Discord не запущен или IPC недоступен",
        ErrorCode::DiscordRpcError => "Discord отклонил активность",
        _ => "неизвестная ошибка Discord",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_not_found_maps_to_not_running() {
        let mapped = from_ipc_error(discord_rich_presence::error::Error::IPCNotFound);
        assert_eq!(mapped.code, ErrorCode::DiscordNotRunning);
    }
}
