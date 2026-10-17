//! Диагностические команды и quit (ТЗ §9.4, §10.1).
//!
//! Отчёт не раскрывает секреты: только классы ошибок, статусы и версии.

use std::fs;
use std::process::Command;

use serde::Serialize;
use tauri::State;

use crate::logging;
use crate::runtime::Supervisor;

/// Очищенный диагностический объект.
#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsReport {
    pub app_version: String,
    pub tauri_version: String,
    pub log_dir: String,
    pub last_error: Option<crate::app_state::ErrorReport>,
    pub ynison_status: String,
    pub discord_status: String,
    pub settings: Option<crate::config::model::AppSettings>,
}

fn status_string<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "unknown".to_string())
}

#[tauri::command]
pub fn get_diagnostics(supervisor: State<'_, Supervisor>) -> DiagnosticsReport {
    let shared = supervisor.shared();
    let snapshot = shared.snapshot();
    DiagnosticsReport {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        tauri_version: tauri::VERSION.to_string(),
        log_dir: logging::log_dir().to_string_lossy().into_owned(),
        last_error: snapshot.last_error,
        ynison_status: status_string(&snapshot.ynison),
        discord_status: status_string(&snapshot.discord),
        settings: Some(shared.settings()),
    }
}

#[tauri::command]
pub fn open_log_directory() -> Result<(), String> {
    let dir = logging::log_dir();
    let _ = fs::create_dir_all(&dir);
    // Открываем только фиксированную папку логов; произвольного пути
    // из фронтенда нет (§10.1).
    Command::new("explorer")
        .arg(dir)
        .spawn()
        .map_err(|err| format!("не удалось открыть папку логов: {err}"))?;
    Ok(())
}

/// Корректная остановка приложения (ТЗ §4.4: явный `Quit`).
#[tauri::command]
pub async fn quit_app(
    app: tauri::AppHandle,
    supervisor: State<'_, Supervisor>,
) -> Result<(), String> {
    supervisor.shutdown_all();
    app.exit(0);
    Ok(())
}
