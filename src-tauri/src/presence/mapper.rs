//! Чистая функция преобразования `PlaybackState` → `DiscordActivity`
//! (ТЗ §6.3–6.6). Никакой сетевой логики — только mapping + правила
//! отображения. Используется координатором.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::model::AppSettings;
use crate::discord::models::DiscordActivity;
use crate::yandex::models::{PlaybackState, PlaybackStatus};

/// Построить Activity по состоянию и настройкам.
///
/// Возвращает `None`, если Presence должен быть очищен (Stopped, пустое
/// состояние, выключенный показ паузы).
pub fn map_presence(
    playback: &PlaybackState,
    settings: &AppSettings,
    now: SystemTime,
) -> Option<DiscordActivity> {
    match playback.status {
        PlaybackStatus::Stopped => None,
        PlaybackStatus::Paused | PlaybackStatus::Unknown if !settings.show_paused_tracks => None,
        PlaybackStatus::Playing | PlaybackStatus::Paused | PlaybackStatus::Unknown => {
            Some(build_activity(playback, settings, now))
        }
    }
}

/// Очистить строки от управляющих символов (без некорректного Unicode).
pub fn sanitize_text(input: &str) -> String {
    input
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect::<String>()
        .trim()
        .to_string()
}

/// Обрезать строку до `max_len` символов по границе графем.
pub fn truncate_text(input: &str, max_len: usize) -> String {
    if input.chars().count() <= max_len {
        return input.to_string();
    }
    let mut result = String::new();
    for ch in input.chars().take(max_len.saturating_sub(1)) {
        result.push(ch);
    }
    result.push('…');
    result
}

fn build_activity(
    playback: &PlaybackState,
    settings: &AppSettings,
    now: SystemTime,
) -> DiscordActivity {
    let mut activity = DiscordActivity::listening();

    activity.details = playback
        .title
        .as_deref()
        .map(sanitize_text)
        .filter(|s| !s.is_empty())
        .map(|s| truncate_text(&s, crate::discord::models::MAX_FIELD_LEN));

    let artists = playback.artists.join(", ");
    let state_text = if artists.is_empty() {
        playback.album.clone().unwrap_or_default()
    } else if playback.status == PlaybackStatus::Paused {
        format!("{artists} (пауза)")
    } else {
        artists
    };
    let state_text = sanitize_text(&state_text);
    activity.state = (!state_text.is_empty())
        .then(|| truncate_text(&state_text, crate::discord::models::MAX_FIELD_LEN));

    // Таймстамп: только при Playing и включённой настройке. На паузе не
    // отправляем таймстамп, чтобы таймер Discord не продолжал ”тикать“.
    if settings.show_track_timestamp && playback.status == PlaybackStatus::Playing {
        if let (Some(position_ms), Ok(elapsed)) = (playback.position_ms, now.duration_since(UNIX_EPOCH))
        {
            let now_secs = elapsed.as_secs() as i64;
            let position_secs = (position_ms / 1000) as i64;
            let start = now_secs.saturating_sub(position_secs);
            activity.start_timestamp = Some(start);
        }
    }

    // Обложка: только если включено отображение и URL прошёл валидацию.
    if settings.show_album_artwork {
        if let Some(cover) = playback.cover_url.as_ref() {
            activity.large_image = Some(cover.as_str().to_string());
            activity.large_text = playback.title.as_deref().map(sanitize_text);
        } else {
            // Fallback на статический asset key (ТЗ §6.7: без unsupported
            // URL — ключ из Developer Portal).
            activity.large_image = Some(crate::discord::models::DEFAULT_ARTWORK_KEY.to_string());
            activity.large_text = playback.title.as_deref().map(sanitize_text);
        }
    }

    // Кнопка: только если URL HTTPS.
    if let Some(track_url) = playback.track_url.as_ref() {
        if track_url.scheme() == "https" {
            activity.button_url = Some(track_url.to_string());
            activity.button_label =
                Some(crate::discord::models::OPEN_IN_YANDEX_LABEL.to_string());
        }
    }

    activity
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discord::models::ACTIVITY_LISTENING;
    use crate::yandex::models::PlaybackState;

    fn now() -> SystemTime {
        UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000)
    }

    fn playing(track_id: &str, title: &str, artists: &[&str], position_ms: u64, duration_ms: u64) -> PlaybackState {
        PlaybackState {
            status: PlaybackStatus::Playing,
            track_id: Some(track_id.to_string()),
            title: Some(title.to_string()),
            artists: artists.iter().map(|s| s.to_string()).collect(),
            album: None,
            duration_ms: Some(duration_ms),
            position_ms: Some(position_ms),
            cover_url: None,
            track_url: None,
            source_device: None,
            observed_at: now(),
        }
    }

    #[test]
    fn playing_maps_all_fields() {
        let settings = AppSettings::default();
        let playback = playing("42", "Тестовый трек", &["A", "B"], 30_000, 200_000);
        let activity = map_presence(&playback, &settings, now()).unwrap();
        assert_eq!(activity.kind, ACTIVITY_LISTENING);
        assert_eq!(activity.details.as_deref(), Some("Тестовый трек"));
        assert_eq!(activity.state.as_deref(), Some("A, B"));
        assert_eq!(activity.start_timestamp, Some(1_699_999_970));
        assert!(activity.large_image.is_some());
        assert!(activity.button_url.is_none());
    }

    #[test]
    fn timestamp_computed_from_position() {
        let settings = AppSettings::default();
        let playback = playing("1", "Track", &["Artist"], 40_000, 100_000);
        let activity = map_presence(&playback, &settings, now()).unwrap();
        // start = now − position (ТЗ §6.5).
        assert_eq!(activity.start_timestamp, Some(1_700_000_000 - 40));
    }

    #[test]
    fn paused_clears_timestamp_and_adds_marker() {
        let settings = AppSettings::default();
        let playback = playing("1", "Track", &["Artist"], 40_000, 100_000);
        let paused = PlaybackState {
            status: PlaybackStatus::Paused,
            ..playback
        };
        let activity = map_presence(&paused, &settings, now()).unwrap();
        assert_eq!(activity.start_timestamp, None, "пауза — без таймстампа");
        assert_eq!(activity.state.as_deref(), Some("Artist (пауза)"));
    }

    #[test]
    fn stopped_clears_activity() {
        let settings = AppSettings::default();
        let playback = playing("1", "Track", &["Artist"], 40_000, 100_000);
        let stopped = PlaybackState {
            status: PlaybackStatus::Stopped,
            ..playback
        };
        assert!(map_presence(&stopped, &settings, now()).is_none());
    }

    #[test]
    fn unknown_treated_like_pause_visibility() {
        let paused_visible = AppSettings::default();
        let paused_hidden = AppSettings {
            show_paused_tracks: false,
            ..AppSettings::default()
        };
        let state = PlaybackState::empty();
        // Unknown при выключенном показе паузы не публикуется.
        assert!(map_presence(&state, &paused_hidden, now()).is_none());
        // Unknown при включённом показе — нейтральная activity без details.
        let activity = map_presence(&state, &paused_visible, now()).unwrap();
        assert_eq!(activity.details, None);
        assert_eq!(activity.state, None);
    }

    #[test]
    fn show_paused_disabled_clears_between_paused() {
        let settings = AppSettings {
            show_paused_tracks: false,
            ..AppSettings::default()
        };
        let playback = playing("1", "Track", &["Artist"], 40_000, 100_000);
        let paused = PlaybackState {
            status: PlaybackStatus::Paused,
            ..playback
        };
        assert!(map_presence(&paused, &settings, now()).is_none());
    }

    #[test]
    fn control_chars_removed() {
        let settings = AppSettings::default();
        let playback = playing("1", "Tra\x0cck", &["Artist"], 0, 100);
        let activity = map_presence(&playback, &settings, now()).unwrap();
        assert_eq!(activity.details.as_deref(), Some("Track"));
    }

    #[test]
    fn https_only_button() {
        let settings = AppSettings::default();
        let mut playback = playing("1", "Track", &["Artist"], 0, 100);
        playback.track_url = Some(url::Url::parse("https://music.yandex.ru/album/1/track/2").unwrap());
        let activity = map_presence(&playback, &settings, now()).unwrap();
        assert_eq!(
            activity.button_url.as_deref(),
            Some("https://music.yandex.ru/album/1/track/2")
        );

        playback.track_url = Some(url::Url::parse("http://music.yandex.ru/track/3").unwrap());
        let activity = map_presence(&playback, &settings, now()).unwrap();
        assert!(activity.button_url.is_none(), "http не допускается");
    }

    #[test]
    fn long_titles_truncated_safely() {
        let settings = AppSettings::default();
        let long_title = "A".repeat(200);
        let playback = playing("1", &long_title, &["Artist"], 0, 100);
        let activity = map_presence(&playback, &settings, now()).unwrap();
        assert!(activity.details.unwrap().chars().count() <= MAX_FIELD_LEN);
    }

    use crate::discord::models::MAX_FIELD_LEN;
}