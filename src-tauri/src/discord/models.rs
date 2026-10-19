//! Модель Discord Activity (ТЗ §6.3). Используется `presence::mapper`
//! для построения и `discord::client` для отправки через IPC.

use serde::{Deserialize, Serialize};

/// Тип активности Listening в Discord RPC.
pub const ACTIVITY_LISTENING: u8 = 2;

/// Ограничения полей Discord RPC (безопасные пределы, ТЗ §6.3).
pub const MAX_FIELD_LEN: usize = 128;

/// Лимит подписи кнопки Discord. Практика показывает, что считается он
/// в байтах UTF-8, хотя официально не уточняется, поэтому длинные
/// кириллические подписи молча гасят всю активность для других.
pub const MAX_BUTTON_LABEL_LEN: usize = 32;

/// Подпись кнопки-ссылки на трек Яндекс Музыки.
/// Короткая осознанно: лимит Discord — 32 единицы, и практика показывает,
/// что длинные кириллические подписи (43 байта у «Открыть в Яндекс Музыке»)
/// гасят всю активность для других пользователей — видимо, лимит считается
/// в байтах. Все подписи ниже влезают и в символы, и в байты (см. тест).
pub const OPEN_IN_YANDEX_LABEL: &str = "Яндекс Музыка";
/// Честная подпись для хитов iTunes (`trackViewUrl`).
pub const OPEN_IN_ITUNES_LABEL: &str = "Открыть в iTunes";
/// Честная подпись для хитов Deezer (`deezer.com/...`).
pub const OPEN_IN_DEEZER_LABEL: &str = "Открыть в Deezer";

/// Подпись кнопки по источнику обогащения (честная: URL и текст совпадают).
pub fn button_label_for_source(source: Option<crate::sources::TrackSource>) -> &'static str {
    match source {
        Some(crate::sources::TrackSource::ITunes) => OPEN_IN_ITUNES_LABEL,
        Some(crate::sources::TrackSource::Deezer) => OPEN_IN_DEEZER_LABEL,
        _ => OPEN_IN_YANDEX_LABEL,
    }
}

/// Значение image key, если обложку нельзя опубликовать динамически.
/// Ключ должен быть загружен в Discord Developer Portal.
pub const DEFAULT_ARTWORK_KEY: &str = "yandex-music-default";

/// Discord Activity с безопасными полями.
///
/// `None` значения не сериализуются — не отправляем пустые строки (ТЗ §6.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscordActivity {
    #[serde(rename = "type")]
    pub kind: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// Unix timestamp старта трека в секундах (только при Playing и
    /// разрешённых настройках).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_timestamp: Option<i64>,
    /// Unix timestamp конца трека (start + duration). Discord рисует
    /// прогресс-бар плеера, когда заданы оба таймстампа (тип Listening).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_timestamp: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub large_image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub large_text: Option<String>,
    /// Максимум одна кнопка в MVP.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub button_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub button_url: Option<String>,
}

impl DiscordActivity {
    pub fn listening() -> Self {
        Self {
            kind: ACTIVITY_LISTENING,
            details: None,
            state: None,
            start_timestamp: None,
            end_timestamp: None,
            large_image: None,
            large_text: None,
            button_label: None,
            button_url: None,
        }
    }
}
