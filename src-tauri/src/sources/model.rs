//! Нормализованная модель состояния воспроизведения (ТЗ §5.3).
//!
//! Модель полностью отвязана от конкретного источника: `yandex::protocol`
//! конвертирует сетевой формат Ynison в эти типы, `sources::smtc_source` —
//! локальные медиа-сессии Windows. Фронтенд и `presence::mapper` работают
//! только с этой моделью. Время хранится в миллисекундах с проверкой на
//! переполнение при парсинге.

use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use url::Url;

/// Статус воспроизведения. `Unknown` означает, что источник не дал
/// достоверного статуса — это не должно приводить к панике.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackStatus {
    Playing,
    Paused,
    Stopped,
    #[default]
    Unknown,
}

/// Нормализованное состояние playback на момент `observed_at`.
/// Сериализуется в camelCase для фронтенда (`usePlaybackClock`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackState {
    pub status: PlaybackStatus,
    /// Непрозрачный идентификатор трека; не предполагаем, что он числовой.
    /// Локальные источники (SMTC) могут его не давать.
    pub track_id: Option<String>,
    pub title: Option<String>,
    /// Список исполнителей, никогда не строка.
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
    /// Позиция воспроизведения на момент `observed_at`, в миллисекундах.
    pub position_ms: Option<u64>,
    /// Нормализованный URL обложки: только HTTPS и allowlist доменов Яндекса.
    /// Локальные источники (SMTC) обложку не отдают (сетевого URL нет).
    pub cover_url: Option<Url>,
    /// Ссылка «Открыть в Яндекс Музыке» (HTTPS, проходящая валидацию).
    pub track_url: Option<Url>,
    /// Название устройства/процесса-источника воспроизведения (для UI).
    /// Для SMTC здесь будет app id медиа-сессии (напр. «Spotify.exe»).
    pub source_device: Option<String>,
    /// Момент, когда источник отдал это состояние.
    #[serde(with = "serde_system_time", default = "SystemTime::now")]
    pub observed_at: SystemTime,
}

impl PlaybackState {
    /// Нейтральное состояние. Используется в тестах mapper.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn empty() -> Self {
        Self {
            status: PlaybackStatus::Unknown,
            track_id: None,
            title: None,
            artists: Vec::new(),
            album: None,
            duration_ms: None,
            position_ms: None,
            cover_url: None,
            track_url: None,
            source_device: None,
            observed_at: SystemTime::now(),
        }
    }

    /// Состояние содержит объект трека, пригодный для отображения.
    /// Используется в тестах mapper/модели.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn has_track(&self) -> bool {
        self.track_id.is_some() || self.title.is_some()
    }
}

/// Поддержка `SystemTime` в serde через unix epoch millis.
mod serde_system_time {
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde::Serializer;

    pub fn serialize<S>(time: &SystemTime, ser: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match time.duration_since(UNIX_EPOCH) {
            Ok(d) => ser.serialize_u64(d.as_millis() as u64),
            Err(_) => ser.serialize_u64(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_state_is_neutral() {
        let state = PlaybackState::empty();
        assert_eq!(state.status, PlaybackStatus::Unknown);
        assert!(state.artists.is_empty());
        assert_eq!(state.track_id, None);
        assert!(!state.has_track());
    }

    #[test]
    fn serialize_roundtrip_keeps_fields_serialized() {
        let mut state = PlaybackState::empty();
        state.status = PlaybackStatus::Playing;
        state.track_id = Some("opaque-id-123".to_string());
        state.artists = vec!["А".into(), "Б".into()];
        state.duration_ms = Some(243_000);
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("opaque-id-123"));
        assert!(json.contains("playing"));
    }

    // Парсинг «сырых» сообщений Ynison в эту модель покрывается тестами
    // `yandex::protocol` на фикстурах.
}
