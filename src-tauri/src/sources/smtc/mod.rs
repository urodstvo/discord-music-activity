//! Windows Media Control (SMTC, GSMTC): локальный fallback-источник
//! (PLAN п.2) + общие COM-хелперы.
//!
//! Состав:
//! - `source` — polling-источник (`spawn_smtc_source`), выбор сессии,
//!   листинг для UI;
//! - `helpers` — блокирующее ожидание WinRT-операций (`await_smtc`) и
//!   сверка ложной паузы Ynison (`smtc_reports_playing`).

pub mod helpers;
pub mod source;

pub use source::{snapshot_sessions_blocking, spawn_smtc_source, MediaSessionInfo};
