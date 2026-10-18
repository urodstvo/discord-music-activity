//! Каноническая модель playback (ТЗ §5.3).
//!
//! Исторически ТЗ §4.1 ожидало `yandex::models`, но модель стала общей для
//! всех источников (Ynison + SMTC), поэтому живёт в `sources::model`.
//! Этот модуль — re-export для совместимости с ТЗ и старыми путями импорта.

#![allow(unused_imports)]

pub use crate::sources::{PlaybackState, PlaybackStatus};
