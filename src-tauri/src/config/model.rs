//! Модель несекретных настроек (ТЗ §8). Токен здесь хранить запрещено.
//!
//! Схема версионируется через `schemaVersion`; при загрузке неверных значений
//! или неизвестной версии приложение не падает — сбрасывается на дефолт.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Тема интерфейса.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

impl fmt::Display for Theme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Theme::System => write!(f, "system"),
            Theme::Light => write!(f, "light"),
            Theme::Dark => write!(f, "dark"),
        }
    }
}

// ser / de через serde с rename_all="lowercase"

/// Язык UI (MVP — только русский, но поле версионируем).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    Ru,
}

/// Несекретные настройки приложения.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    /// Версия схемы для миграций. Изменение требует migration step.
    pub schema_version: u32,
    pub presence_enabled: bool,
    pub show_paused_tracks: bool,
    pub show_track_timestamp: bool,
    pub show_album_artwork: bool,
    pub start_on_launch: bool,
    pub close_to_tray: bool,
    pub minimize_to_tray: bool,
    pub theme: Theme,
    pub language: Language,
    pub diagnostics_enabled: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: 1,
            presence_enabled: true,
            show_paused_tracks: true,
            show_track_timestamp: true,
            show_album_artwork: true,
            start_on_launch: false,
            close_to_tray: true,
            minimize_to_tray: true,
            theme: Theme::default(),
            language: Language::default(),
            diagnostics_enabled: true,
        }
    }
}

impl AppSettings {
    /// Санитизация настроек после загрузки: invalid значения не должны
    /// ломать запуск. Возвращает очищенную копию.
    pub fn sanitized(mut self) -> Self {
        if self.schema_version == 0 || self.schema_version > 1 {
            // Неизвестная версия — сбрасываем на текущую, сохраняя поля.
            tracing::warn!(
                "неизвестная schemaVersion={} — сбрасываю на 1",
                self.schema_version
            );
            self.schema_version = 1;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_spec() {
        let s = AppSettings::default();
        assert!(s.presence_enabled);
        assert!(s.close_to_tray);
        assert!(!s.start_on_launch);
        assert_eq!(s.theme, Theme::System);
        assert_eq!(s.language, Language::Ru);
        assert_eq!(s.schema_version, 1);
    }

    #[test]
    fn deserializes_spec_example() {
        let json = r#"{
            "schemaVersion": 1,
            "presenceEnabled": true,
            "showPausedTracks": true,
            "showTrackTimestamp": true,
            "showAlbumArtwork": true,
            "startOnLaunch": false,
            "closeToTray": true,
            "minimizeToTray": true,
            "theme": "system",
            "language": "ru",
            "diagnosticsEnabled": true
        }"#;
        // Требуется camelCase сериализация, проверяется serialization attrs.
        let parsed: Result<AppSettings, _> = serde_json::from_str(json);
        assert!(parsed.is_ok(), "должен парситься camelCase-вариант");
    }

    #[test]
    fn unknown_version_falls_back() {
        let s = AppSettings {
            schema_version: 9,
            ..AppSettings::default()
        }
        .sanitized();
        assert_eq!(s.schema_version, 1);
    }

    #[test]
    fn unknown_fields_do_not_break_loading() {
        let json = r#"{"schemaVersion": 1, "presenceEnabled": false, "totally_new_field": 42}"#;
        let parsed: AppSettings = serde_json::from_str(json).unwrap();
        assert!(!parsed.presence_enabled);
        assert_eq!(parsed.theme, Theme::System);
    }
}
