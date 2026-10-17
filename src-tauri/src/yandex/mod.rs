//! Интеграция с Яндекс Музыкой: авторизация, Ynison, метаданные.
//!
//! Модуль не выполняет поиск треков по строкам и не участвует в загрузке
//! аудио. Основной источник состояния — долгоживущее WebSocket-соединение
//! Ynison; REST используется только для недостающих метаданных по track ID.

pub mod auth;
pub mod errors;
pub mod models;
pub mod metadata;
pub mod normalize;
pub mod protocol;
pub mod ynison_client;
