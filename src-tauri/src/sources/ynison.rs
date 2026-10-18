//! Ynison как источник `sources::SourceEvent`.
//!
//! Протокол, handshake, keep-alive и разбор кадров живут в
//! `yandex::ynison_client` (специфика Яндекс Музыки). Этот модуль — тонкий
//! адаптер, чтобы `sources/` перечислял ВСЕ источники: `runtime::Supervisor`
//! спавнит Ynison отсюда, а не напрямую из `yandex/`.

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::app_state::AppSharedState;
use crate::sources::SourceEvent;

/// Запустить Ynison-источник как фоновую задачу (сигнатура — как у
/// `sources::smtc::spawn_smtc_source`, чтобы арбитр работал единообразно).
pub fn spawn_ynison_source(
    token: String,
    shared: Arc<AppSharedState>,
    app_handle: tauri::AppHandle,
    shutdown: CancellationToken,
    events: mpsc::UnboundedSender<SourceEvent>,
) {
    crate::yandex::ynison_client::spawn_ynison_client(token, shared, app_handle, shutdown, events);
}
