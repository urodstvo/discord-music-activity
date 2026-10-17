//! Команды подключения и playback (ТЗ §10.1).

use tauri::State;

use crate::app_state::AppStateSnapshot;
use crate::runtime::Supervisor;

/// Безопасный снапшот состояния, без секретов.
#[tauri::command]
pub fn get_app_state(supervisor: State<'_, Supervisor>) -> AppStateSnapshot {
    supervisor.shared().snapshot()
}

/// Явная команда переподключения Ynison.
#[tauri::command]
pub fn reconnect_yandex(supervisor: State<'_, Supervisor>, app: tauri::AppHandle) {
    supervisor.reconnect_yandex(&app);
}

/// Пересоздать Discord IPC подключение.
#[tauri::command]
pub fn reconnect_discord(supervisor: State<'_, Supervisor>, app: tauri::AppHandle) {
    supervisor.reconnect_discord(&app);
}
