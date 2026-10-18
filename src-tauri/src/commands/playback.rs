//! Команды подключения и playback (ТЗ §10.1).

use tauri::State;

use crate::app_state::AppStateSnapshot;
use crate::runtime::Supervisor;
use crate::sources::smtc_source::MediaSessionInfo;

/// Безопасный снапшот состояния, без секретов.
#[tauri::command]
pub fn get_app_state(supervisor: State<'_, Supervisor>) -> AppStateSnapshot {
    supervisor.shared().snapshot()
}

/// Видимые сейчас медиа-сессии Windows (для фиксации процесса, PLAN п.2.1).
/// Только отображаемые поля (app id, заголовок, исполнители, статус) —
/// секретов нет. Выполняется в blocking-потоке с таймаутом: COM-вызовы
/// нельзя делать в async-контексте Tauri-команды.
#[tauri::command]
pub async fn list_media_sessions() -> Vec<MediaSessionInfo> {
    let outcome = tokio::time::timeout(
        std::time::Duration::from_millis(3_000),
        tokio::task::spawn_blocking(crate::sources::smtc_source::snapshot_sessions_blocking),
    )
    .await;
    match outcome {
        Ok(Ok(sessions)) => sessions,
        _ => Vec::new(),
    }
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
