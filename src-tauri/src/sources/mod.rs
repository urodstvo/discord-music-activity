//! Единый контракт источников данных о воспроизведении (PLAN п.2: fallback
//! через Windows Media Control).
//!
//! Каждый источник:
//! 1. публикует состояние ТОЛЬКО в нормализованной модели (`model::PlaybackState`),
//! 2. отправляет события общего вида `SourceEvent` в канал Supervisor'а,
//! 3. гасится через `CancellationToken`.
//!
//! Координатор Presence, mapper и UI работают только с нормализованной
//! моделью и не знают, откуда пришло состояние. Новый источник = новый
//! модуль с той же сигнатурой `spawn_*` + регистрация в Supervisor'е
//! (`runtime.rs`), остальной код не меняется.

pub mod model;
pub mod smtc_source;

// Ynison-клиент физически живёт в `yandex/` (протокол специфичен для
// Яндекса), но события отдаёт в общий контракт `SourceEvent`.

use serde::Serialize;

pub use model::{PlaybackState, PlaybackStatus};

/// Класс источника данных. Используется для арбитража в Supervisor'е и
/// отображения активного источника в UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// Ynison (WebSocket Яндекс Музыки) — основной, точный источник.
    #[default]
    Ynison,
    /// Windows Media Control (SMTC) — локальный fallback.
    MediaControl,
}
/// События источника для Supervisor'а.
#[derive(Debug)]
pub enum SourceEvent {
    /// Нормализованное состояние (`None` — воспроизведение не обнаружено).
    State {
        kind: SourceKind,
        state: Option<PlaybackState>,
    },
    /// Токен отклонён сервером (только Ynison).
    RequiresAuth,
    /// Источник остановлен (shutdown или терминальная ошибка).
    Stopped { kind: SourceKind },
}
