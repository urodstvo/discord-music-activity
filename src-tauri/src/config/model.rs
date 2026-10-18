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

/// Источник данных о воспроизведении (PLAN п.2: fallback через
/// Windows Media Control).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SourceMode {
    /// Ynison (токен) приоритетен; без токена/при сбоях — SMTC fallback.
    #[default]
    Auto,
    /// Только Ynison: без токена — ничего (как раньше).
    Ynison,
    /// Только Windows Media Control: работает без авторизации.
    MediaControl,
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
    /// Режим источника playback (см. PLAN п.2).
    pub source_mode: SourceMode,
    /// Фиксация SMTC-источника на конкретном процессе (PLAN п.2.1).
    /// Пустая строка — авто (sticky + деприоритезация браузеров).
    /// Иначе точный app id медиа-сессии (напр. `Spotify.exe`):
    /// другие процессы игнорируются, пока закрепление задано.
    #[serde(default)]
    pub smtc_source_app: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: 3,
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
            source_mode: SourceMode::default(),
            smtc_source_app: String::new(),
        }
    }
}

impl AppSettings {
    /// Санитизация настроек после загрузки: invalid значения не должны
    /// ломать запуск. Возвращает очищенную копию. Миграции:
    /// v1 → v2 добавляет `sourceMode`; v2 → v3 добавляет `smtcSourceApp`.
    pub fn sanitized(mut self) -> Self {
        if self.schema_version == 1 {
            // v1 не знал про source_mode: serde default уже проставил Auto.
            tracing::info!("миграция настроек: schemaVersion 1 → 2 (+sourceMode=auto)");
            self.schema_version = 2;
        }
        if self.schema_version == 2 {
            // v2 не знал про фиксацию SMTC-процесса: авто-режим.
            self.smtc_source_app = String::new();
            self.schema_version = 3;
        }
        if self.schema_version == 0 || self.schema_version > 3 {
            // Неизвестная версия — сбрасываем на текущую, сохраняя поля.
            tracing::warn!(
                "неизвестная schemaVersion={} — сбрасываю на 3",
                self.schema_version
            );
            self.schema_version = 3;
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
        assert_eq!(s.schema_version, 3);
        assert_eq!(s.source_mode, SourceMode::Auto);
        assert!(s.smtc_source_app.is_empty(), "по умолчанию — авто");
    }

    #[test]
    fn v1_config_migrates_to_v2() {
        // Конфиг v1 без sourceMode: serde default ставит Auto, sanitized
        // поднимает версию схемы.
        let mut s: AppSettings =
            serde_json::from_str(r#"{"schemaVersion": 1, "presenceEnabled": false}"#).unwrap();
        assert_eq!(s.source_mode, SourceMode::Auto);
        s = s.sanitized();
        assert_eq!(s.schema_version, 3);
        assert!(!s.presence_enabled, "значения v1 сохраняются");
    }

    #[test]
    fn v2_config_migrates_to_v3() {
        let mut s: AppSettings =
            serde_json::from_str(r#"{"schemaVersion": 2, "presenceEnabled": true}"#).unwrap();
        assert!(s.smtc_source_app.is_empty());
        s = s.sanitized();
        assert_eq!(s.schema_version, 3);
        assert!(s.presence_enabled, "значения v2 сохраняются");
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
        assert_eq!(s.schema_version, 3);
    }

    #[test]
    fn unknown_fields_do_not_break_loading() {
        let json = r#"{"schemaVersion": 1, "presenceEnabled": false, "totally_new_field": 42}"#;
        let parsed: AppSettings = serde_json::from_str(json).unwrap();
        assert!(!parsed.presence_enabled);
        assert_eq!(parsed.theme, Theme::System);
    }
}
