//! Windows Media Control (SMTC, GSMTC): локальный fallback-источник
//! (PLAN п.2) + общие COM-хелперы.
//!
//! Состав:
//! - `source` — polling-источник (`spawn_smtc_source`), выбор сессии,
//!   листинг для UI;
//! - `helpers` — блокирующее ожидание WinRT-операций (`await_smtc`) и
//!   сверка ложной паузы Ynison (`smtc_reports_playing`);
//! - `matching` — строгая проверка совпадений (порт `ymrpc/matching.py`);
//! - `catalogs` — каскад Yandex → iTunes → Deezer (порт `ymrpc/catalogs.py`);
//! - `enrichment` — фоновая догонка обложки/ссылки на `Change::Identity`.

pub mod catalogs;
pub mod enrichment;
pub mod helpers;
pub mod matching;
pub mod source;

pub use source::{MediaSessionInfo, snapshot_sessions_blocking, spawn_smtc_source};
