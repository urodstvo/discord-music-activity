//! Команды работы с токеном (ТЗ §5.1, §10.1).
//!
//! Токен передаётся только в Rust; не возвращается наружу и не логируется.

use tauri::State;

use crate::runtime::Supervisor;
use crate::yandex::auth;
use crate::yandex::errors::{AppError, AppResult, ErrorCode};

/// Проверить токен без сохранения. Возвращает безопасный результат.
#[tauri::command]
pub async fn validate_yandex_token(token: String) -> AppResult<()> {
    auth::validate_token(&token).await
}

/// Сохранить токен в системное хранилище и запустить подключение.
#[tauri::command]
pub async fn save_yandex_token(
    token: String,
    supervisor: State<'_, Supervisor>,
    app: tauri::AppHandle,
) -> AppResult<()> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return Err(AppError::new(ErrorCode::NoToken));
    }
    // Сначала проверяем — не создаём постоянных сетевых циклов (§5.1).
    auth::validate_token(trimmed).await?;
    auth::save_token(trimmed)?;
    supervisor.start_ynison(&app);
    let shared = supervisor.shared();
    shared.notify(&app, "info", "Токен сохранён, подключаюсь к Яндекс Музыке");
    Ok(())
}

/// Удалить токен: остановить синхронизацию, очистить Presence (§5.1).
#[tauri::command]
pub async fn remove_yandex_token(
    supervisor: State<'_, Supervisor>,
    app: tauri::AppHandle,
) -> AppResult<()> {
    // 1. Остановить Ynison (координатор очистит Activity).
    supervisor.stop_ynison(&app);
    // 2. Забрать токен из credential store.
    auth::delete_token().inspect_err(|err| {
        let shared = supervisor.shared();
        shared.push_error(err, &app);
    })?;
    // 3. Обновить снапшот: токен отсутствует.
    let shared = supervisor.shared();
    shared.notify(&app, "info", "Токен удалён");
    Ok(())
}

/// Требуется реальный статус авторизации: наличие токена в хранилище.
#[tauri::command]
pub fn get_auth_status(supervisor: State<'_, Supervisor>) -> bool {
    let shared = supervisor.shared();
    let snapshot = shared.snapshot();
    snapshot.auth_required
}
