//! Координация Presence: единая точка, решающая, когда обновлять или
//! очищать Discord Activity (ТЗ §7).

pub mod backoff;
pub mod coordinator;
pub mod grace;
pub mod mapper;
pub mod smtc;
