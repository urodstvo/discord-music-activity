//! Интеграция с Discord Rich Presence через локальный IPC.
//!
//! Adapter изолирован: протокол IPC не распространяется на остальной код.

pub mod client;
pub mod models;
pub mod errors;
