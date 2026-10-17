//! Tauri команды. Узкий API для фронтенда — без произвольного доступа к
//! файловой системе/секретам (ТЗ §10.1).

pub mod auth;
pub mod diagnostics;
pub mod playback;
pub mod settings;
