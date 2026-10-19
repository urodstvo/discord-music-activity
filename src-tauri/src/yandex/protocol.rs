//! Ynison protocol types and playback-state normalization.
//!
//! The protocol is JSON over WebSocket (a betterproto JSON projection of a
//! gRPC-ish service). Server frames are full snapshots (never deltas):
//! each `PutYnisonStateResponse` carries the whole active player state, so
//! parsing leniently and treating each message as authoritative is correct.
//!
//! Tolerant decoding is mandatory: unknown/secondary fields must not break
//! parsing (ТЗ §4.2, §5.2).

use std::time::{SystemTime, UNIX_EPOCH};

use crate::sources::{PlaybackState, PlaybackStatus};
use serde::Deserialize;
use url::Url;

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
#[allow(dead_code)] // поля — часть толерантной схемы протокола
pub struct VersionBlock {
    #[serde(default, alias = "timestamp_ms")]
    pub timestamp_ms: Option<NumField>,
    #[serde(default, alias = "device_id")]
    pub device_id: Option<String>,
}

/// Track queue: contains the active playable.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)] // entity_* — часть протокола, читается не всеми путями
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
#[allow(dead_code)] // playable_type — часть протокола
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
/// Возвращает `None`, если в кадре нет данных о воспроизведении.
pub fn normalize(frame: &YnisonFrame, now: SystemTime) -> Option<PlaybackState> {
    // Очередь: верхний уровень ИЛИ вложенная в player_state (продакшен).
    let queue = frame
        .player_queue
        .as_ref()
        .or_else(|| frame.player_state.as_ref()?.player_queue.as_ref())?;
    let playable = active_playable(queue)?;
    let state = frame.player_state.as_ref()?;

    let (status, block) = status_of(state);
    let track_id = playable.playable_id.as_ref().and_then(NumField::as_string);
    let cover_url = playable
        .cover_url_optional
        .as_deref()
        .and_then(|tpl| sanitize_cover(tpl, "400x400"));

    let mut normalized = PlaybackState {
        status,
        track_id,
        title: playable.title.clone(),
        artists: Vec::new(),
        album: None,
        duration_ms: parse_u64_field(&block.duration_ms),
        position_ms: position_from_status(block, now),
        cover_url,
        track_url: None,
        source_device: source_device_title(frame),
        observed_at: now,
    };

    // track_url построим только при наличии непрозрачного track id.
    if let Some(id) = normalized.track_id.clone() {
        normalized.track_url = build_track_url(&id);
    }

    Some(normalized)
}

/// Активный playable из очереди.
fn active_playable(queue: &PlayerQueue) -> Option<&Playable> {
    let list = queue.playable_list.as_ref()?;
    let index = queue
        .current_playable_index
        .as_ref()
        .and_then(|v| v.as_i64())?;
    // Отрицательный индекс = очередь пуста.
    if index < 0 {
        return None;
    }
    list.get(usize::try_from(index).ok()?)
}

/// Статус + блок колонок прогресса для математики позиции.
fn status_of(state: &PlayerState) -> (PlaybackStatus, &StatusBlock) {
    // Пустой блок прогресса — «статус отсутствует».
    const EMPTY_STATUS: StatusBlock = StatusBlock {
        paused: None,
        progress_ms: None,
        duration_ms: None,
        playback_speed: None,
        version: None,
    };
    let Some(status_block) = state.status.as_ref() else {
        return (PlaybackStatus::Stopped, &EMPTY_STATUS);
    };
    let paused = status_block.paused.unwrap_or(false);
    let status = if paused {
        PlaybackStatus::Paused
    } else {
        PlaybackStatus::Playing
    };
    (status, status_block)
}

/// Позиция с учётом поддерживающих полей (ТЗ §5.2: position = progress +
/// elapsed × speed, пауза замораживает).
fn position_from_status(block: &StatusBlock, now: SystemTime) -> Option<u64> {
    let progress_ms = parse_u64_field(&block.progress_ms)?;
    let speed = block
        .playback_speed
        .as_ref()
        .and_then(NumField::as_f64)
        .unwrap_or(1.0)
        .max(0.0);
    let stamp_ms = block
        .version
        .as_ref()
        .and_then(|v| parse_u64_field(&v.timestamp_ms))
        .unwrap_or(0);
    let now_ms = now
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let duration_ms = parse_u64_field(&block.duration_ms).unwrap_or(0);

    Some(normalized_position(
        progress_ms,
        block.paused.unwrap_or(false),
        speed,
        stamp_ms,
        now_ms,
        duration_ms,
    ))
}

/// Ссылка «Открыть в Яндекс Музыке» по непрозрачному track id.
pub fn build_track_url(track_id: &str) -> Option<Url> {
    if track_id.trim().is_empty() || track_id.len() > 64 {
        return None;
    }
    Url::parse(&format!("https://music.yandex.ru/track/{track_id}")).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn now() -> SystemTime {
        UNIX_EPOCH + std::time::Duration::from_millis(1_700_000_000_000)
    }

    fn playing_frame() -> serde_json::Value {
        json!({
            "playerState": {
                "status": {
                    "paused": false,
                    "playbackSpeed": 1.0,
                    "progressMs": "30000",
                    "durationMs": "200000",
                    "version": {"deviceId": "dev1", "timestampMs": "1699999970000"}
                }
            },
            "playerQueue": {
                "currentPlayableIndex": 0,
                "playableList": [{
                    "playableId": "94817057",
                    "playableType": "TRACK",
                    "title": "Track 1",
                    "coverUrlOptional": "https://avatars.yandex.net/get-music-content/1/2/%%"
                }]
            }
        })
    }

    #[test]
    fn playing_frame_normalizes() {
        let frame = parse_frame(&playing_frame()).unwrap();
        // Version stamp = now-30s, speed 1 → позиция 30000+30000=60000.
        let state = normalize(&frame, now()).unwrap();
        assert_eq!(state.status, PlaybackStatus::Playing);
        assert_eq!(state.track_id.as_deref(), Some("94817057"));
        assert_eq!(state.title.as_deref(), Some("Track 1"));
        assert_eq!(state.position_ms, Some(60_000));
        assert!(state.cover_url.is_some());
        assert!(
            state
                .track_url
                .as_ref()
                .map(|u| u.as_str().contains("music.yandex.ru/track/"))
                .unwrap_or(false)
        );
    }

    #[test]
    fn paused_freezes_position() {
        let mut frame = playing_frame();
        frame["playerState"]["status"]["paused"] = json!(true);
        let frame = parse_frame(&frame).unwrap();
        let state = normalize(&frame, now()).unwrap();
        assert_eq!(state.status, PlaybackStatus::Paused);
        assert_eq!(
            state.position_ms,
            Some(30_000),
            "пауза замораживает позицию"
        );
    }

    #[test]
    fn unknown_extra_fields_are_tolerated() {
        let mut frame = playing_frame();
        frame["unknownServiceGrowth"] = json!({"a": 1, "deep": {"n": [1, 2]}});
        frame["playerQueue"]["brandNewField"] = json!(true);
        let frame = parse_frame(&frame).unwrap();
        assert!(normalize(&frame, now()).is_some());
    }

    #[test]
    fn snake_case_frame_with_numbers_normalizes() {
        // Продакшен-сервер шлёт кадры snake_case и int64 числом (как
        // redirect-кадр). Раньше это парсилось в «всё None» — пустую очередь.
        let json = json!({
            "player_state": {
                "status": {
                    "paused": false,
                    "playback_speed": 1.0,
                    "progress_ms": 30000,
                    "duration_ms": 200000,
                    "version": {"device_id": "dev1", "timestamp_ms": 1699999970000u64}
                }
            },
            "player_queue": {
                "current_playable_index": 0,
                "playable_list": [{
                    "playable_id": 94817057,
                    "playable_type": "TRACK",
                    "title": "Track 1",
                    "cover_url_optional": "https://avatars.yandex.net/get-music-content/1/2/%%"
                }]
            }
        });
        let frame = parse_frame(&json).unwrap();
        let state = normalize(&frame, now()).unwrap();
        assert_eq!(state.status, PlaybackStatus::Playing);
        assert_eq!(state.track_id.as_deref(), Some("94817057"));
        assert_eq!(state.title.as_deref(), Some("Track 1"));
        assert_eq!(state.position_ms, Some(60_000));
        assert!(state.cover_url.is_some());
    }

    #[test]
    fn nested_queue_in_player_state_normalizes() {
        // Продакшен: player_queue вложен ВНУТРЬ player_state, а не рядом.
        let json = json!({
            "player_state": {
                "status": {"paused": false},
                "player_queue": {
                    "current_playable_index": 0,
                    "playable_list": [{"playable_id": "123", "title": "Nested"}]
                }
            },
            "devices": []
        });
        let frame = parse_frame(&json).unwrap();
        let state = normalize(&frame, now()).unwrap();
        assert_eq!(state.title.as_deref(), Some("Nested"));
        assert_eq!(state.status, PlaybackStatus::Playing);
    }

    #[test]
    fn empty_queue_returns_none_without_panic() {
        let json = json!({
            "playerState": {"status": {"paused": true}},
            "playerQueue": {"currentPlayableIndex": -1, "playableList": []}
        });
        let frame = parse_frame(&json).unwrap();
        assert!(normalize(&frame, now()).is_none());
    }

    #[test]
    fn missing_metadata_does_not_panic() {
        let json = json!({
            "playerQueue": {"currentPlayableIndex": 0, "playableList": [{}]},
            "playerState": {"status": {}}
        });
        let frame = parse_frame(&json).unwrap();
        let state = normalize(&frame, now()).unwrap();
        assert!(state.track_id.is_none());
        assert!(state.title.is_none());
        assert_eq!(state.status, PlaybackStatus::Playing);
    }

    #[test]
    fn speed_2x_advances_double() {
        let mut frame = playing_frame();
        frame["playerState"]["status"]["playbackSpeed"] = json!(2.0);
        let frame = parse_frame(&frame).unwrap();
        let state = normalize(&frame, now()).unwrap();
        assert_eq!(state.position_ms, Some(30_000 + 2 * 30_000));
    }

    #[test]
    fn cover_rejects_non_https_and_foreign_hosts() {
        let json = json!({
            "playerState": {"status": {"paused": true}},
            "playerQueue": {"currentPlayableIndex": 0, "playableList": [{
                "playableId": "1",
                "coverUrlOptional": "http://evil.example/%%"
            }]}
        });
        let frame = parse_frame(&json).unwrap();
        let state = normalize(&frame, now()).unwrap();
        assert!(state.cover_url.is_none(), "//http не допускается");

        let json = json!({
            "playerState": {"status": {"paused": true}},
            "playerQueue": {"currentPlayableIndex": 0, "playableList": [{
                "playableId": "1",
                "coverUrlOptional": "data:image/png;base64,AAAA"
            }]}
        });
        let frame = parse_frame(&json).unwrap();
        let state = normalize(&frame, now()).unwrap();
        assert!(state.cover_url.is_none(), "data: не допускается");
    }

    #[test]
    fn absent_playback_state_winds_up_neutral() {
        // Нет playerState вовсе — normalize вернёт None (нет данных).
        let json = json!({
            "playerQueue": {"currentPlayableIndex": 0, "playableList": [{"playableId": "1"}]}
        });
        let frame = parse_frame(&json).unwrap();
        assert!(normalize(&frame, now()).is_none());

        // Пустой playerState — интерпретируется как Stopped.
        let json = json!({
            "playerState": {},
            "playerQueue": {"currentPlayableIndex": 0, "playableList": [{"playableId": "1"}]}
        });
        let frame = parse_frame(&json).unwrap();
        let state = normalize(&frame, now()).unwrap();
        assert_eq!(state.status, PlaybackStatus::Stopped);
    }

    #[test]
    fn track_url_rejects_bad_ids() {
        assert!(build_track_url("").is_none());
        assert!(build_track_url("   ").is_none());
        assert!(build_track_url(&"a".repeat(65)).is_none());
        assert!(build_track_url("94817057").is_some());
    }
}
