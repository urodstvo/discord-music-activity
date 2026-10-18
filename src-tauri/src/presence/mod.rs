//! Координация Presence: единая точка, решающая, когда обновлять или
//! очищать Discord Activity (ТЗ §7).
//!
//! SMTC-хелперы живут в `crate::sources::smtc` (источник + `await_smtc` +
//! сверка ложной паузы); здесь — только решение publish/clear.

pub mod backoff;
pub mod coordinator;
pub mod grace;
pub mod mapper;
