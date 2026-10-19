//! Классы ошибок приложения (см. ТЗ §11.2).
//!
//! Ошибки реализованы как типизированная модель, из которой `commands`/frontend
//! получают код + человекочитаемое описание. Никакие секреты здесь не хранятся
//! и не форматируются в сообщения.

use std::fmt;

/// Категория ошибки, устойчивая к изменению формата сообщений.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Токен не введён.
    NoToken,
    /// Токен отклонён сервером.
    InvalidToken,
    /// Токен истёк — требуется повторная авторизация.
    TokenExpired,
    /// Яндекс Музыка / сеть недоступны.
    YandexUnavailable,
    /// Ответ Ynison не соответствует ожидаемой схеме.
    YnisonProtocolChanged,
    /// Discord не запущен / IPC недоступен.
    DiscordNotRunning,
    /// Discord отклонил Activity.
    DiscordRpcError,
    /// Обложка недоступна (не фатально).
    ArtworkUnavailable,
    /// Настройки не читаются/не пишутся.
    SettingsReadWriteError,
    /// Системное хранилище секретов недоступно.
    SecretStoreError,
    /// Код device flow истёк — нужно начать вход заново.
    DeviceAuthExpired,
    /// Пользователь отклонил доступ на странице Яндекса.
    DeviceAuthDenied,
    /// Внутренняя ошибка, не покрываемая другими классами.
    Internal,
}

impl ErrorCode {
    /// Человекочитаемое описание по-русски, предназначенное для UI.
    pub fn ui_text(self) -> &'static str {
        match self {
            ErrorCode::NoToken => "Токен не задан",
            ErrorCode::InvalidToken => "Токен не принят",
            ErrorCode::TokenExpired => "Требуется повторная авторизация",
            ErrorCode::YandexUnavailable => "Сеть или сервис Яндекс Музыки недоступны",
            ErrorCode::YnisonProtocolChanged => {
                "Протокол Яндекс Музыки изменился — обновите приложение"
            }
            ErrorCode::DiscordNotRunning => "Discord не запущен",
            ErrorCode::DiscordRpcError => "Discord отклонил обновление статуса",
            ErrorCode::ArtworkUnavailable => "Обложка недоступна",
            ErrorCode::SettingsReadWriteError => "Не удалось сохранить настройки",
            ErrorCode::SecretStoreError => "Системное хранилище секретов недоступно",
            ErrorCode::DeviceAuthExpired => "Код входа истёк — начните заново",
            ErrorCode::DeviceAuthDenied => "Доступ отклонён на странице Яндекса",
            ErrorCode::Internal => "Внутренняя ошибка",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.ui_text())
    }
}

/// Ошибка уровня приложения.
///
/// В конструкторах запрещено передавать секреты: `details` не должен
/// содержать токен. Для Tauri commands используются сериализованные
/// форма́ты (code + человекочитаемое сообщение), details наружу не уходит.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppError {
    pub code: ErrorCode,
    pub details: Option<String>,
}

// Сериализация для IPC: наружу уходим только code + ui_text.
impl serde::Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut st = serializer.serialize_struct("AppError", 2)?;
        st.serialize_field("code", &self.code)?;
        st.serialize_field("message", self.code.ui_text())?;
        st.end()
    }
}

impl AppError {
    pub fn new(code: ErrorCode) -> Self {
        Self {
            code,
            details: None,
        }
    }

    pub fn with_details(code: ErrorCode, details: impl Into<String>) -> Self {
        Self {
            code,
            details: Some(details.into()),
        }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.code.ui_text())
    }
}

impl std::error::Error for AppError {}

pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_does_not_leak_details() {
        let err = AppError::with_details(ErrorCode::InvalidToken, "raw server answer");
        assert_eq!(err.to_string(), "Токен не принят");
    }

    #[test]
    fn error_codes_serialize_snake_case() {
        let json = serde_json::to_string(&ErrorCode::YnisonProtocolChanged).unwrap();
        assert_eq!(json, r#""ynison_protocol_changed""#);
    }
}
