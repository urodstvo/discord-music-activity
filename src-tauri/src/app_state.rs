//! Прикладное состояние: настройки, снапшот соединений, обработка ошибок.
//!
//! Здесь нет секретов: OAuth-токен живёт только в credential store и
//! передаётся внутренним адаптерам по каналу. Изменения состояния
//! публикуются фронту через события Tauri (`app-state-changed` и другие,
//! ТЗ §10.2).

use std::sync::RwLock;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::config::model::AppSettings;
use crate::yandex::errors::AppError;
use crate::yandex::models::PlaybackState;

/// Статус соединения Ynison (ТЗ §5.2 — полный жизненный цикл).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum YnisonStatus {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    AuthenticationRequired,
    ProtocolError,
}

/// Статус соединения с Discord IPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DiscordStatus {
    #[default]
    Disconnected,
    Connecting,
    Connected,
}

/// Отчёт об ошибке для UI и диагностики (без секретов).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorReport {
    pub code: crate::yandex::errors::ErrorCode,
    pub message: String,
    pub at_unix_ms: u64,
}

impl ErrorReport {
    pub fn from_app_error(err: &AppError) -> Self {
        Self {
            code: err.code,
            message: err.code.ui_text().to_string(),
            at_unix_ms: unix_millis_now(),
        }
    }
}

pub fn unix_millis_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Снапшот состояния для `get_app_state`. Не содержит секретов.
/// Поля сериализуются в camelCase — фронтенд ожидает именно его
/// (иначе authRequired на UI был undefined и карта токена врала).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStateSnapshot {
    pub ynison: YnisonStatus,
    pub discord: DiscordStatus,
    pub auth_required: bool,
    pub playback: Option<PlaybackState>,
    pub presence_enabled: bool,
    pub last_error: Option<ErrorReport>,
}

// Безопасные defaults: считаем авторизацию требуемой (safe(§9.2), пока
// не подтверждено обратное).
impl Default for AppStateSnapshot {
    fn default() -> Self {
        Self {
            ynison: YnisonStatus::default(),
            discord: DiscordStatus::default(),
            auth_required: true,
            playback: None,
            presence_enabled: true,
            last_error: None,
        }
    }
}

/// Состояние, публично доступное командам Tauri и фоновым задачам.
pub struct AppSharedState {
    settings: RwLock<AppSettings>,
    snapshot: RwLock<AppStateSnapshot>,
    /// Глобальный cancellation token — гасит все долгоживущие задачи (§4.3).
    pub shutdown: tokio_util::sync::CancellationToken,
}

impl AppSharedState {
    pub fn new(settings: AppSettings) -> Self {
        let presence_enabled = settings.presence_enabled;
        Self {
            snapshot: RwLock::new(AppStateSnapshot {
                presence_enabled,
                ..AppStateSnapshot::default()
            }),
            settings: RwLock::new(settings),
            shutdown: tokio_util::sync::CancellationToken::new(),
        }
    }

    pub fn settings(&self) -> AppSettings {
        self.settings.read().expect("settings доступен").clone()
    }

    pub fn update_settings(&self, settings: AppSettings) {
        let presence_enabled = settings.presence_enabled;
        *self.settings.write().expect("settings доступен") = settings;
        {
            let mut snap = self.snapshot.write().expect("snapshot доступен");
            snap.presence_enabled = presence_enabled;
        }
    }

    /// Инициализировать `auth_required` по факту наличия токена в хранилище
    /// (вызывается один раз при старте приложения). Иначе фронтенд до
    /// первого статуса Ynison всегда показывает карточку «введите токен» —
    /// из-за этого токен приходилось вводить заново при каждом запуске.
    pub fn set_token_present(&self, present: bool) {
        let mut snap = self.snapshot.write().expect("snapshot доступен");
        snap.auth_required = !present;
    }

    pub fn snapshot(&self) -> AppStateSnapshot {
        self.snapshot.read().expect("snapshot доступен").clone()
    }

    /// Обновить снапшот и уведомить frontend. Если окна нет — просто не
    /// доставим событие, ошибки нет.
    pub fn publish_snapshot(&self, app: &AppHandle) {
        let snap = self.snapshot.read().expect("snapshot доступен").clone();
        let _ = app.emit("app-state-changed", &snap);
    }

    pub fn set_ynison_status(&self, status: YnisonStatus, app: &AppHandle) {
        {
            let mut snap = self.snapshot.write().expect("snapshot доступен");
            snap.ynison = status;
            if status == YnisonStatus::AuthenticationRequired
                || status == YnisonStatus::Disconnected
            {
                snap.auth_required = true;
            } else if status == YnisonStatus::Connected {
                snap.auth_required = false;
            }
        }
        self.publish_snapshot(app);
    }

    pub fn set_discord_status(&self, status: DiscordStatus, app: &AppHandle) {
        {
            let mut snap = self.snapshot.write().expect("snapshot доступен");
            snap.discord = status;
        }
        self.publish_snapshot(app);
    }

    /// Записать последнюю ошибку в снапшот и послать UI notification.
    pub fn push_error(&self, err: &AppError, app: &AppHandle) {
        {
            let mut snap = self.snapshot.write().expect("snapshot доступен");
            snap.last_error = Some(ErrorReport::from_app_error(err));
        }
        self.publish_snapshot(app);
        self.notify(app, "error", err.code.ui_text());
    }

    /// Обновить нормализованное состояние playback (вызывается из Ynison).
    pub fn push_playback(&self, playback: Option<PlaybackState>, app: &AppHandle) {
        {
            let mut snap = self.snapshot.write().expect("snapshot доступен");
            snap.playback = playback.clone();
        }
        self.publish_snapshot(app);
        let _ = app.emit("playback-changed", &playback);
    }
    /// Безопасное уведомление уровня info/warning/error (§10.2).
    pub fn notify(&self, app: &AppHandle, level: &str, message: &str) {
        let _ = app.emit(
            "notification",
            serde_json::json!({ "level": level, "message": message }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_defaults_are_safe() {
        let snap = AppStateSnapshot::default();
        assert!(snap.auth_required);
        assert!(snap.playback.is_none());
        assert!(snap.presence_enabled);
    }

    #[test]
    fn settings_roundtrip_via_state() {
        let mut settings = AppSettings::default();
        settings.presence_enabled = false;
        let state = AppSharedState::new(settings.clone());
        state.update_settings(settings);
        assert!(!state.snapshot().presence_enabled);
    }
}
