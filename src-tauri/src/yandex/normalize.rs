//! Нормализация кадров Ynison в `PlaybackState` (ТЗ §5.3).
//!
//! Сервер присылает полные снапшоты; позиция вычисляется от точки отсчёта
//! `status.version.timestampMs` с учётом `playbackSpeed` и паузы. Отсутствие
//! второстепенных полей обрабатывается нейтрально, без паник.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::yandex::models::{PlaybackState, PlaybackStatus};
use crate::yandex::protocol::{
    parse_u64_field, sanitize_cover, source_device_title, NumField, PlayerQueue, PlayerState,
    YnisonFrame,
};

/// Единая точка входа: кадр → нормализованное состояние (или None, если
/// в кадре нет данных о воспроизведении).
pub struct NormalizeFrame;

impl NormalizeFrame {
    pub fn normalize(frame: &YnisonFrame, now: SystemTime) -> Option<PlaybackState> {
        // Очередь: верхний уровень ИЛИ вложенная в player_state (продакшен).
        let queue = frame
            .player_queue
            .as_ref()
            .or_else(|| frame.player_state.as_ref()?.player_queue.as_ref())?;
        let playable = Self::active_playable(queue)?;
        let state = frame.player_state.as_ref()?;

        let (status, block) = Self::status_of(state, now);
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
            position_ms: Self::normalized_position(block, now),
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
    fn active_playable(queue: &PlayerQueue) -> Option<&crate::yandex::protocol::Playable> {
        let list = queue.playable_list.as_ref()?;
        let index = queue.current_playable_index.as_ref().and_then(|v| v.as_i64())?;
        // Отрицательный индекс = очередь пуста.
        if index < 0 {
            return None;
        }
        list.get(usize::try_from(index).ok()?)
    }

    /// Статус + блок колонок прогресса для математики позиции.
    fn status_of<'a>(
        state: &'a PlayerState,
        _now: SystemTime,
    ) -> (PlaybackStatus, &'a crate::yandex::protocol::StatusBlock) {
        // Пустой блок прогресса — «статус отсутствует».
        const EMPTY_STATUS: crate::yandex::protocol::StatusBlock =
            crate::yandex::protocol::StatusBlock {
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

    /// Позиция с учётом подтеживающих полей (ТЗ §5.2: position = progress +
    /// elapsed × speed, пауза замораживает).
    fn normalized_position(
        block: &crate::yandex::protocol::StatusBlock,
        now: SystemTime,
    ) -> Option<u64> {
        use crate::yandex::protocol::NumField;
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

        Some(crate::yandex::protocol::normalized_position(
            progress_ms,
            block.paused.unwrap_or(false),
            speed,
            stamp_ms,
            now_ms,
            duration_ms,
        ))
    }
}

/// Ссылка «Открыть в Яндекс Музыке» по непрозрачному track id.
pub fn build_track_url(track_id: &str) -> Option<url::Url> {
    if track_id.trim().is_empty() || track_id.len() > 64 {
        return None;
    }
    url::Url::parse(&format!("https://music.yandex.ru/track/{track_id}")).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::yandex::protocol::parse_frame;
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
        let state = NormalizeFrame::normalize(&frame, now()).unwrap();
        assert_eq!(state.status, PlaybackStatus::Playing);
        assert_eq!(state.track_id.as_deref(), Some("94817057"));
        assert_eq!(state.title.as_deref(), Some("Track 1"));
        assert_eq!(state.position_ms, Some(60_000));
        assert!(state.cover_url.is_some());
        assert!(state
            .track_url
            .as_ref()
            .map(|u| u.as_str().contains("music.yandex.ru/track/"))
            .unwrap_or(false));
    }

    #[test]
    fn paused_freezes_position() {
        let mut frame = playing_frame();
        frame["playerState"]["status"]["paused"] = json!(true);
        let frame = parse_frame(&frame).unwrap();
        let state = NormalizeFrame::normalize(&frame, now()).unwrap();
        assert_eq!(state.status, PlaybackStatus::Paused);
        assert_eq!(state.position_ms, Some(30_000), "пауза замораживает позицию");
    }

    #[test]
    fn unknown_extra_fields_are_tolerated() {
        let mut frame = playing_frame();
        frame["unknownServiceGrowth"] = json!({"a": 1, "deep": {"n": [1, 2]}});
        frame["playerQueue"]["brandNewField"] = json!(true);
        let frame = parse_frame(&frame).unwrap();
        assert!(NormalizeFrame::normalize(&frame, now()).is_some());
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
        let state = NormalizeFrame::normalize(&frame, now()).unwrap();
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
        let state = NormalizeFrame::normalize(&frame, now()).unwrap();
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
        assert!(NormalizeFrame::normalize(&frame, now()).is_none());
    }

    #[test]
    fn missing_metadata_does_not_panic() {
        let json = json!({
            "playerQueue": {"currentPlayableIndex": 0, "playableList": [{}]},
            "playerState": {"status": {}}
        });
        let frame = parse_frame(&json).unwrap();
        let state = NormalizeFrame::normalize(&frame, now()).unwrap();
        assert!(state.track_id.is_none());
        assert!(state.title.is_none());
        assert_eq!(state.status, PlaybackStatus::Playing);
    }

    #[test]
    fn speed_2x_advances_double() {
        let mut frame = playing_frame();
        frame["playerState"]["status"]["playbackSpeed"] = json!(2.0);
        let frame = parse_frame(&frame).unwrap();
        let state = NormalizeFrame::normalize(&frame, now()).unwrap();
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
        let state = NormalizeFrame::normalize(&frame, now()).unwrap();
        assert!(state.cover_url.is_none(), "//http не допускается");

        let json = json!({
            "playerState": {"status": {"paused": true}},
            "playerQueue": {"currentPlayableIndex": 0, "playableList": [{
                "playableId": "1",
                "coverUrlOptional": "data:image/png;base64,AAAA"
            }]}
        });
        let frame = parse_frame(&json).unwrap();
        let state = NormalizeFrame::normalize(&frame, now()).unwrap();
        assert!(state.cover_url.is_none(), "data: не допускается");
    }

    #[test]
    fn absent_playback_state_winds_up_neutral() {
        // Нет playerState вовсе — normalize неpublish None (нет данных).
        let json = json!({
            "playerQueue": {"currentPlayableIndex": 0, "playableList": [{"playableId": "1"}]}
        });
        let frame = parse_frame(&json).unwrap();
        assert!(NormalizeFrame::normalize(&frame, now()).is_none());

        // Пустой playerState — интерпретируется как Stopped.
        let json = json!({
            "playerState": {},
            "playerQueue": {"currentPlayableIndex": 0, "playableList": [{"playableId": "1"}]}
        });
        let frame = parse_frame(&json).unwrap();
        let state = NormalizeFrame::normalize(&frame, now()).unwrap();
        assert_eq!(state.status, PlaybackStatus::Stopped);
    }
}
