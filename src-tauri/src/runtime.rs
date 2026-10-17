//! Управление долгоживущими задачами (ТЗ §4.3: managed application state /
//! task manager).
//!
//! Supervisor владеет жизненным циклом Ynison клиента и каналом событий
//! координатора. Задачи аккуратно гасятся через cancellation token.

use std::sync::Arc;
use std::sync::Mutex;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::app_state::{AppSharedState, DiscordStatus, YnisonStatus};
use crate::yandex::auth;
use crate::yandex::ynison_client::{spawn_ynison_client, YnisonEvent as ClientEvent};

/// События для координатора.
#[derive(Debug)]
pub enum CoordinatorEvent {
    /// Нормализованное состояние (None — остановка).
    PlaybackState(Option<crate::yandex::models::PlaybackState>),
    /// Токен отклонён сервером.
    RequiresAuth,
    /// Клиент Ynison остановлен.
    Stopped,
    /// Пользователь запросил пересоздание Discord IPC (ТЗ §10.1).
    DiscordReset,
}

impl From<ClientEvent> for CoordinatorEvent {
    fn from(event: ClientEvent) -> Self {
        match event {
            ClientEvent::PlaybackState(state) => CoordinatorEvent::PlaybackState(state),
            ClientEvent::RequiresAuth => CoordinatorEvent::RequiresAuth,
            ClientEvent::Stopped => CoordinatorEvent::Stopped,
        }
    }
}

/// Управление Ynison клиентом и Discord reset.
pub struct Supervisor {
    shared: Arc<AppSharedState>,
    coordinator_tx: mpsc::UnboundedSender<CoordinatorEvent>,
    /// Child token текущей задачи Ynison.
    ynison_shutdown: Mutex<Option<CancellationToken>>,
}

impl Supervisor {
    pub fn shared(&self) -> Arc<AppSharedState> {
        Arc::clone(&self.shared)
    }
    pub fn new(
        shared: Arc<AppSharedState>,
    ) -> (Self, mpsc::UnboundedReceiver<CoordinatorEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Self {
                shared,
                coordinator_tx: tx,
                ynison_shutdown: Mutex::new(None),
            },
            rx,
        )
    }

    /// Запустить Ynison, если токен есть. Существующий клиент останавливается.
    pub fn start_ynison(&self, app: &tauri::AppHandle) {
        self.stop_ynison(app);

        let token = match auth::load_token() {
            Ok(Some(token)) => token,
            Ok(None) => {
                tracing::info!("start_ynison: токена нет — клиент не запущен");
                return;
            }
            Err(err) => {
                self.shared.push_error(&err, app);
                return;
            }
        };

        // События клиента → CoordinatorEvent → координатор (один хоп).
        let child = self.shared.shutdown.child_token();
        let receiver = spawn_ynison_client(
            token,
            Arc::clone(&self.shared),
            app.clone(),
            child.clone(),
        );
        let coordinator_tx = self.coordinator_tx.clone();
        tauri::async_runtime::spawn(async move {
            let mut receiver = receiver;
            while let Some(event) = receiver.recv().await {
                if coordinator_tx.send(event.into()).is_err() {
                    break;
                }
            }
        });

        *self
            .ynison_shutdown
            .lock()
            .expect("supervisor доступен") = Some(child);
        self.shared
            .set_ynison_status(YnisonStatus::Connecting, app);
    }

    /// Остановить Ynison клиент.
    pub fn stop_ynison(&self, app: &tauri::AppHandle) {
        if let Some(token) = self
            .ynison_shutdown
            .lock()
            .expect("supervisor доступен")
            .take()
        {
            token.cancel();
        }
        let _ = self.coordinator_tx.send(CoordinatorEvent::Stopped);
        self.shared
            .set_ynison_status(YnisonStatus::Disconnected, app);
    }

    /// Явная команда переподключения (ТЗ §10.1).
    pub fn reconnect_yandex(&self, app: &tauri::AppHandle) {
        self.start_ynison(app);
    }

    /// Пересоздать Discord IPC подключение (ТЗ §10.1).
    pub fn reconnect_discord(&self, app: &tauri::AppHandle) {
        self.shared
            .set_discord_status(DiscordStatus::Connecting, app);
        let _ = self.coordinator_tx.send(CoordinatorEvent::DiscordReset);
    }

    /// Полная остановка всех фоновых задач (ТЗ §6.4: quit/logout).
    pub fn shutdown_all(&self) {
        if let Some(token) = self
            .ynison_shutdown
            .lock()
            .expect("supervisor доступен")
            .take()
        {
            token.cancel();
        }
        self.shared.shutdown.cancel();
    }
}
