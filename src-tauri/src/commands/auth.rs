//! Команды работы с токеном (ТЗ §5.1, §10.1).
//!
//! Токен передаётся только в Rust; не возвращается наружу и не логируется.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, State};

use crate::runtime::Supervisor;
use crate::yandex::auth;
use crate::yandex::device_auth::{self, DeviceCodeInfo, PollOutcome};
use crate::yandex::errors::{AppError, AppResult, ErrorCode};

/// Состояние фонового опроса Device Flow: отмена предыдущего входа при
/// повторном старте или явной отмене из UI.
pub struct DeviceAuthManager {
    current: Mutex<Option<tokio_util::sync::CancellationToken>>,
}

impl Default for DeviceAuthManager {
    fn default() -> Self {
        Self {
            current: Mutex::new(None),
        }
    }
}

impl DeviceAuthManager {
    /// Отменить предыдущую сессию входа, если она ещё идёт.
    fn cancel_previous(&self) {
        if let Some(token) = self.current.lock().expect("device auth доступен").take() {
            token.cancel();
        }
    }
}

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
    supervisor.on_token_saved(&app);
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
    // 1. Забрать токен из credential store.
    auth::delete_token().inspect_err(|err| {
        let shared = supervisor.shared();
        shared.push_error(err, &app);
    })?;
    // 2. Пересчитать источники: арбитр остановит Ynison и, в режиме auto,
    //    подхватит SMTC fallback (без токена).
    supervisor.on_token_removed(&app);
    // 3. Уведомить UI.
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

/// Начать вход через Яндекс (Device Flow): вернуть `user_code` для UI и
/// запустить фоновый опрос токена. Предыдущий незавершённый вход отменяется.
#[tauri::command]
pub async fn start_device_auth(
    supervisor: State<'_, Supervisor>,
    manager: State<'_, DeviceAuthManager>,
    app: AppHandle,
) -> AppResult<DeviceCodeInfo> {
    manager.cancel_previous();
    let (info, device_code) = device_auth::request_device_code().await?;
    let cancel = tokio_util::sync::CancellationToken::new();
    *manager.current.lock().expect("device auth доступен") = Some(cancel.clone());

    let supervisor_shared = supervisor.shared();
    let supervisor_owned: Supervisor = (*supervisor).clone();
    let app_handle = app.clone();
    let interval = Duration::from_secs(info.interval_secs);
    let deadline = Instant::now() + Duration::from_secs(info.expires_in_secs);
    tauri::async_runtime::spawn(async move {
        let mut wait = interval;
        loop {
            if cancel.is_cancelled() {
                tracing::info!("device auth отменён пользователем");
                break;
            }
            if Instant::now() >= deadline {
                let err = AppError::new(ErrorCode::DeviceAuthExpired);
                supervisor_shared.push_error(&err, &app_handle);
                let _ = app_handle.emit("device-auth-error", &err);
                break;
            }
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(wait) => {}
            }
            if cancel.is_cancelled() {
                break;
            }
            match device_auth::poll_device_token_once(&device_code).await {
                Ok(PollOutcome::Pending) => {
                    // Сервер просил снизить темп через slow_down — опрос
                    // продолжается, следующий интервал чуть длиннее.
                    wait = interval + Duration::from_secs(2);
                }
                Ok(PollOutcome::Authorized(token)) => {
                    // Токен из device flow проверяем тем же путём, что и
                    // ручной, и только потом сохраняем (ТЗ §5.1).
                    if let Err(err) = auth::validate_token(&token).await {
                        supervisor_shared.push_error(&err, &app_handle);
                        let _ = app_handle.emit("device-auth-error", &err);
                        break;
                    }
                    if let Err(err) = auth::save_token(&token) {
                        supervisor_shared.push_error(&err, &app_handle);
                        let _ = app_handle.emit("device-auth-error", &err);
                        break;
                    }
                    supervisor_owned.on_token_saved(&app_handle);
                    supervisor_shared
                        .notify(&app_handle, "info", "Вход через Яндекс выполнен");
                    let _ = app_handle.emit("device-auth-done", serde_json::json!({}));
                    break;
                }
                Err(err) => {
                    supervisor_shared.push_error(&err, &app_handle);
                    let _ = app_handle.emit("device-auth-error", &err);
                    break;
                }
            }
        }
        // Сессия завершена любым исходом: помечаем свой токен и чистим слот,
        // но только если там всё ещё он (повторный старт не сбрасываем).
        cancel.cancel();
        let mgr = app_handle.state::<DeviceAuthManager>();
        let mut slot = mgr.current.lock().expect("device auth доступен");
        if slot.as_ref().map(|t| t.is_cancelled()).unwrap_or(false) {
            *slot = None;
        }
    });

    Ok(info)
}

/// Отменить ожидание подтверждения на странице Яндекса.
#[tauri::command]
pub fn cancel_device_auth(manager: State<'_, DeviceAuthManager>) {
    manager.cancel_previous();
}
