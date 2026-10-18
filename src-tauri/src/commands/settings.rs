//! Команды настроек (ТЗ §8, §10.1). Только несекретные значения.

use serde_json::{Map, Value};
use tauri::{AppHandle, Emitter, State};

use crate::config::model::AppSettings;
use crate::config::persistence::save_settings;
use crate::logging;
use crate::runtime::Supervisor;

/// Текущие настройки.
#[tauri::command]
pub fn get_settings(supervisor: State<'_, Supervisor>) -> AppSettings {
    supervisor.shared().settings()
}

/// Провалидировать и сохранить частичное обновление настроек.
/// Принимаются только известные поля; секреты здесь не хранятся.
#[tauri::command]
pub async fn update_settings(
    partial: Map<String, Value>,
    supervisor: State<'_, Supervisor>,
    app: AppHandle,
) -> Result<AppSettings, String> {
    let mut current = supervisor.shared().settings();
    match merged(&mut current, &partial) {
        Ok(()) => {}
        Err(err) => return Err(err),
    }

    // Сохранение на диск тоже может провалиться — не теряем in-memory.
    match save_settings(&crate::config::persistence::settings_path(), &current) {
        Ok(()) => {}
        Err(err) => return Err(err.code.ui_text().to_string()),
    }
    supervisor.shared().update_settings(current.clone());
    logging::set_verbose(current.diagnostics_enabled);
    crate::tray::update_tray(&app, current.presence_enabled);
    apply_autostart(&app, current.start_on_launch);
    // Источники playback могут зависеть от настроек (sourceMode) —
    // арбитр пересчитает, перезапуска будет только при изменениях.
    supervisor.settings_changed(&app);
    let _ = app.emit("settings-changed", &current);
    Ok(current)
}

/// Атомарно обновить присенс-флаг и пересчитать состояние (ТЗ §10.1).
#[tauri::command]
pub async fn set_presence_enabled(
    enabled: bool,
    supervisor: State<'_, Supervisor>,
    app: AppHandle,
) -> Result<AppSettings, String> {
    let mut current = supervisor.shared().settings();
    current.presence_enabled = enabled;
    match save_settings(&crate::config::persistence::settings_path(), &current) {
        Ok(()) => {}
        Err(err) => return Err(err.code.ui_text().to_string()),
    }
    supervisor.shared().update_settings(current.clone());
    crate::tray::update_tray(&app, current.presence_enabled);
    let _ = app.emit("settings-changed", &current);
    Ok(current)
}

/// Применить известные поля `partial` к настройкам.
fn merged(current: &mut AppSettings, partial: &Map<String, Value>) -> Result<(), String> {
    for (key, value) in partial {
        match key.as_str() {
            "presenceEnabled" => merge_bool(&mut current.presence_enabled, value, key)?,
            "showPausedTracks" => merge_bool(&mut current.show_paused_tracks, value, key)?,
            "showTrackTimestamp" => {
                merge_bool(&mut current.show_track_timestamp, value, key)?
            }
            "showAlbumArtwork" => merge_bool(&mut current.show_album_artwork, value, key)?,
            "startOnLaunch" => merge_bool(&mut current.start_on_launch, value, key)?,
            "closeToTray" => merge_bool(&mut current.close_to_tray, value, key)?,
            "minimizeToTray" => merge_bool(&mut current.minimize_to_tray, value, key)?,
            "theme" => merge_theme(&mut current.theme, value, key)?,
            "sourceMode" => merge_source_mode(&mut current.source_mode, value, key)?,
            "smtcSourceApp" => merge_smtc_source_app(&mut current.smtc_source_app, value, key)?,
            "diagnosticsEnabled" => {
                merge_bool(&mut current.diagnostics_enabled, value, key)?
            }
            _ => {
                // Неизвестные настройки игнорируем (не ломаем совместимость).
            }
        }
    }
    Ok(())
}

fn merge_bool(target: &mut bool, value: &Value, key: &str) -> Result<(), String> {
    match value.as_bool() {
        Some(b) => {
            *target = b;
            Ok(())
        }
        None => Err(format!("{key}: ожидается bool")),
    }
}

fn merge_theme(
    target: &mut crate::config::model::Theme,
    value: &Value,
    key: &str,
) -> Result<(), String> {
    match value.as_str() {
        Some("system") => {
            *target = crate::config::model::Theme::System;
            Ok(())
        }
        Some("light") => {
            *target = crate::config::model::Theme::Light;
            Ok(())
        }
        Some("dark") => {
            *target = crate::config::model::Theme::Dark;
            Ok(())
        }
        _ => Err(format!("{key}: ожидается system|light|dark")),
    }
}
fn merge_source_mode(
    target: &mut crate::config::model::SourceMode,
    value: &Value,
    key: &str,
) -> Result<(), String> {
    match value.as_str() {
        Some("auto") => {
            *target = crate::config::model::SourceMode::Auto;
            Ok(())
        }
        Some("ynison") => {
            *target = crate::config::model::SourceMode::Ynison;
            Ok(())
        }
        Some("media_control") => {
            *target = crate::config::model::SourceMode::MediaControl;
            Ok(())
        }
        _ => Err(format!(
            "{key}: ожидается auto|ynison|media_control"
        )),
    }
}

/// Закрепление SMTC-процесса: пустая строка — авто-режим,
/// иначе точный app id (обрезаем пробелы, лимит длины).
fn merge_smtc_source_app(target: &mut String, value: &Value, key: &str) -> Result<(), String> {
    match value.as_str() {
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.len() > 128 {
                return Err(format!("{key}: слишком длинный app id"));
            }
            *target = trimmed.to_string();
            Ok(())
        }
        None => Err(format!("{key}: ожидается строка")),
    }
}

// Применение автозапуска через tauri-plugin-autostart (ТЗ §8).
fn apply_autostart(app: &AppHandle, enabled: bool) {
    use tauri::Manager;
    use tauri_plugin_autostart::AutoLaunchManager;
    match app.try_state::<AutoLaunchManager>() {
        Some(autostart) => {
            let result = if enabled {
                autostart.enable()
            } else {
                autostart.disable()
            };
            if let Err(err) = result {
                // Не считаем фатальной ошибкой: настройка сохранена,
                // регистрация может быть недоступна (политики ОС).
                tracing::warn!("autostart переключить не удалось");
                let _ = err;
            }
        }
        None => {
            tracing::warn!("autostart плагин недоступен");
        }
    }
}
