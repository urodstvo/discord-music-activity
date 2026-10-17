//! Ynison protocol types and playback-state normalization.
//!
//! The protocol is JSON over WebSocket (a betterproto JSON projection of a
//! gRPC-ish service). Server frames are full snapshots (never deltas):
//! each `PutYnisonStateResponse` carries the whole active player state, so
//! parsing leniently and treating each message as authoritative is correct.
//!
//! Tolerant decoding is mandatory: unknown/secondary fields must not break
//! parsing (ТЗ §4.2, §5.2).

use std::time::SystemTime;

use serde::Deserialize;
use url::Url;
use crate::yandex::models::PlaybackState;
use crate::yandex::normalize::NormalizeFrame;

/// Maximum raw frame size we are willing to keep in memory (64 MiB, mirroring
/// the reference clients — frames are quasi-full snapshots and stay small).
pub const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

/// Allowed cover-CDN hosts for `cover_url` (ТЗ §5.3 allows customization).
pub const ALLOWED_COVER_HOSTS: &[&str] = &["avatars.yandex.net", "avatars.mds.yandex.net"];

/// Числовое поле protobuf (int64/float), приходящее И строкой, И числом:
/// продакшен-сервер (и snake_case-сериализация лучшеproto) допускает обе формы.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum NumField {
    Int(i64),
    Float(f64),
    Str(String),
}

impl NumField {
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            NumField::Int(v) => Some(*v),
            NumField::Float(v) => Some(*v as i64),
            NumField::Str(s) => s.trim().parse().ok(),
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        self.as_i64().and_then(|v| u64::try_from(v).ok())
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            NumField::Int(v) => Some(*v as f64),
            NumField::Float(v) => Some(*v),
            NumField::Str(s) => s.trim().parse().ok(),
        }
    }

    pub fn as_string(&self) -> Option<String> {
        match self {
            NumField::Str(s) => Some(s.clone()),
            NumField::Int(v) => Some(v.to_string()),
            NumField::Float(v) => Some(format!("{v}")),
        }
    }
}

/// Raw server frame shape (fields we care about; unknown ones are ignored).
///
/// ВАЖНО: сервер присылает УЖЕ snake_case (как redirect-кадр), но исторические
/// клиентские примеры — camelCase. Каждый первичный ключ camelCase имеет
/// alias snake_case; числовые поля — `NumField` (строка или число).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct YnisonFrame {
    #[serde(default, alias = "player_state")]
    pub player_state: Option<PlayerState>,
    #[serde(default, alias = "player_queue")]
    pub player_queue: Option<PlayerQueue>,
    #[serde(default)]
    pub devices: Option<Vec<DeviceBlock>>,
    #[serde(default, alias = "active_device_id_optional")]
    pub active_device_id_optional: Option<String>,
}

/// Playback status/progress block.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerState {
    #[serde(default)]
    pub status: Option<StatusBlock>,
    /// В продакшене очередь вложена В player_state (proto PutYnisonState
    /// .PlayerState { player_queue, status }); на верхний уровень тоже
    /// принимаем (форма канонического фейк-сервера).
    #[serde(default, alias = "player_queue")]
    pub player_queue: Option<PlayerQueue>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusBlock {
    #[serde(default)]
    pub paused: Option<bool>,
    /// int64 приходит строкой (camelCase-вариант) или числом (snake_case).
    #[serde(default, alias = "progress_ms")]
    pub progress_ms: Option<NumField>,
    #[serde(default, alias = "duration_ms")]
    pub duration_ms: Option<NumField>,
    #[serde(default, alias = "playback_speed")]
    pub playback_speed: Option<NumField>,
    #[serde(default)]
    pub version: Option<VersionBlock>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionBlock {
    #[serde(default, alias = "timestamp_ms")]
    pub timestamp_ms: Option<NumField>,
    #[serde(default, alias = "device_id")]
    pub device_id: Option<String>,
}

/// Track queue: contains the active playable.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerQueue {
    #[serde(default, alias = "current_playable_index")]
    pub current_playable_index: Option<NumField>,
    #[serde(default, alias = "playable_list")]
    pub playable_list: Option<Vec<Playable>>,
    #[serde(default)]
    pub entity_type: Option<String>,
    #[serde(default)]
    pub entity_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Playable {
    #[serde(default, alias = "playable_id")]
    pub playable_id: Option<NumField>,
    #[serde(default, alias = "playable_type")]
    pub playable_type: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default, alias = "cover_url_optional")]
    pub cover_url_optional: Option<String>,
}

/// Device list attached to the frame (used to expose `source_device`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceBlock {
    #[serde(default)]
    pub device: Option<DeviceInner>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInner {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
}

/// The source device title, matched against `active_device_id_optional`.
pub(crate) fn source_device_title(frame: &YnisonFrame) -> Option<String> {
    let active = frame.active_device_id_optional.as_ref()?;
    let devices = frame.devices.as_ref()?;
    devices
        .iter()
        .find(|dev| dev.device.as_ref().and_then(|d| d.id.as_deref()) == Some(active.as_str()))
        .and_then(|dev| dev.device.as_ref().and_then(|d| d.title.clone()))
}

/// Parse a JSON protobuf frame from the current server.
pub fn parse_frame(json: &serde_json::Value) -> Option<YnisonFrame> {
    // Leniently decode; on structual mismatch we prefer to log and return
    // None rather than propagate incompatible frames into the coordinator.
    serde_json::from_value::<YnisonFrame>(json.clone()).ok()
}

/// Convert server-frame numeric fields (string OR number) to u64.
pub(crate) fn parse_u64_field(value: &Option<NumField>) -> Option<u64> {
    value.as_ref().and_then(NumField::as_u64)
}

pub(crate) fn normalized_position(
    progress_ms: u64,
    paused: bool,
    playback_speed: f64,
    version_stamp_ms: u64,
    now_ms: u64,
    duration_ms: u64,
) -> u64 {
    if paused {
        return progress_ms;
    }
    let elapsed_ms = now_ms.saturating_sub(version_stamp_ms);
    // If the clock drifted absurdly (stray oversized timestamp/hours-long gap),
    // fall back to reporting the raw progress instead of inventing time.
    if elapsed_ms > 24 * 3600 * 1000 {
        return progress_ms;
    }
    let delta = predicted_delta(elapsed_ms, playback_speed);
    progress_ms.saturating_add(delta).min(duration_ms)
}

fn predicted_delta(elapsed_ms: u64, playback_speed: f64) -> u64 {
    if !playback_speed.is_finite() || playback_speed <= 0.0 {
        return 0;
    }
    let product = elapsed_ms as f64 * playback_speed;
    if !product.is_finite() || product > u64::MAX as f64 {
        return 0;
    }
    product as u64
}

/// Build a normalized URL only when it is HTTPS and the host is allowed
/// (ТЗ §5.3: block file:, javascript:, data:, internal paths).
pub(crate) fn sanitize_cover(template: &str, size: &str) -> Option<Url> {
    let substituted = template.replace("%%", size);
    let url = Url::parse(&substituted).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    let host = url.host_str()?;
    if !ALLOWED_COVER_HOSTS.contains(&host) {
        return None;
    }
    Some(url)
}

/// Normalize any concrete `YnisonFrame` into a UI- and Presence-ready state.
pub fn normalize(frame: &YnisonFrame, now: SystemTime) -> Option<PlaybackState> {
    NormalizeFrame::normalize(frame, now)
}
