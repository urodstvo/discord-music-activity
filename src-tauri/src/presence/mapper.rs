//! Чистая функция преобразования `PlaybackState` → `DiscordActivity`
//! (ТЗ §6.3–6.6). Никакой сетевой логики — только mapping + правила
//! отображения. Используется координатором.

use std::time::UNIX_EPOCH;

use crate::config::model::AppSettings;
use crate::discord::models::DiscordActivity;
use crate::sources::{PlaybackState, PlaybackStatus};

/// Построить Activity по состоянию и настройкам.
///
/// Возвращает `None`, если Presence должен быть очищен (Stopped, пустое
/// состояние, выключенный показ паузы).
pub fn map_presence(playback: &PlaybackState, settings: &AppSettings) -> Option<DiscordActivity> {
    match playback.status {
        PlaybackStatus::Stopped => None,
        PlaybackStatus::Paused | PlaybackStatus::Unknown if !settings.show_paused_tracks => None,
        PlaybackStatus::Playing | PlaybackStatus::Paused | PlaybackStatus::Unknown => {
            Some(build_activity(playback, settings))
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

fn build_activity(playback: &PlaybackState, settings: &AppSettings) -> DiscordActivity {
    let mut activity = DiscordActivity::listening();

    // Верхняя строка — всегда название трека.
    activity.details = playback
        .title
        .as_deref()
        .map(sanitize_text)
        .filter(|s| !s.is_empty())
        .map(|s| truncate_text(&s, crate::discord::models::MAX_FIELD_LEN));

    // Вторая строка — только исполнители (альбом уехал в large_text —
    // третью строку карточки Discord).
    let state_text = sanitize_text(&playback.artists.join(", "));
    // Хвост паузы: «Artist (пауза)»; при пустой строке — «(пауза)».
    let state_text = if playback.status == PlaybackStatus::Paused {
        if state_text.is_empty() {
            "(пауза)".to_string()
        } else {
            format!("{state_text} (пауза)")
        }
    } else {
        state_text
    };
    activity.state = (!state_text.is_empty())
        .then(|| truncate_text(&state_text, crate::discord::models::MAX_FIELD_LEN));

    // Таймстампы: только при Playing и включённой настройке. На паузе не
    // отправляем таймстамп, чтобы таймер Discord не продолжал ”тикать“.
    // Оба таймстампа (start + end) рисуют в Discord прогресс-бар плеера.
    // База — момент замера позиции (`observed_at`), а не момент отправки:
    // иначе debounce/rate-limit координатора (до 4.5 c) сдвигал бы старт
    // вперёд и Discord отставал бы от карточки приложения.
    if settings.show_track_timestamp && playback.status == PlaybackStatus::Playing {
        if let (Some(position_ms), Ok(elapsed)) = (
            playback.position_ms,
            playback.observed_at.duration_since(UNIX_EPOCH),
        ) {
            let now_secs = elapsed.as_secs() as i64;
            let position_secs = (position_ms / 1000) as i64;
            let start = now_secs.saturating_sub(position_secs);
            activity.start_timestamp = Some(start);
            if let Some(duration_ms) = playback.duration_ms {
                activity.end_timestamp = Some(start + (duration_ms / 1000) as i64);
            }
        }
    }

    // Обложка: только если включено отображение и URL прошёл валидацию.
    // Третья строка карточки (large_text) — всегда альбом, а не название:
    // название уже заняло первую строку (details).
    let large_text = playback
        .album
        .as_deref()
        .map(sanitize_text)
        .filter(|s| !s.is_empty())
        .map(|s| truncate_text(&s, crate::discord::models::MAX_FIELD_LEN));
    if settings.show_album_artwork {
        if let Some(cover) = playback.cover_url.as_ref() {
            activity.large_image = Some(cover.as_str().to_string());
            activity.large_text = large_text;
        } else {
            // Fallback на статический asset key (ТЗ §6.7: без unsupported
            // URL — ключ из Developer Portal).
            activity.large_image = Some(crate::discord::models::DEFAULT_ARTWORK_KEY.to_string());
            activity.large_text = large_text;
        }
    }

    // Кнопка: только если включена и URL HTTPS. Подпись честная по источнику
    // (`track_source`): Яндекс/iTunes/Deezer — URL и текст совпадают.
    // Legacy Ynison без `track_source` — дефолт Яндекс (обратная совместимость).
    if settings.show_button
        && playback
            .track_url
            .as_ref()
            .is_some_and(|u| u.scheme() == "https")
    {
        activity.button_url = Some(playback.track_url.as_ref().unwrap().to_string());
        activity.button_label = Some(
            crate::discord::models::button_label_for_source(playback.track_source).to_string(),
        );
    }

    activity
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::*;
    use crate::discord::models::ACTIVITY_LISTENING;
    use crate::sources::PlaybackState;

    fn now() -> SystemTime {
        UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000)
    }

    fn playing(
        track_id: &str,
        title: &str,
        artists: &[&str],
        position_ms: u64,
        duration_ms: u64,
    ) -> PlaybackState {
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
            track_source: None,
            source_device: None,
            observed_at: now(),
        }
    }

    #[test]
    fn playing_maps_all_fields() {
        let settings = AppSettings::default();
        let playback = playing("42", "Тестовый трек", &["A", "B"], 30_000, 200_000);
        let activity = map_presence(&playback, &settings).unwrap();
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
        let activity = map_presence(&playback, &settings).unwrap();
        // start = now − position; end = start + duration (прогресс-бар).
        assert_eq!(activity.start_timestamp, Some(1_700_000_000 - 40));
        assert_eq!(activity.end_timestamp, Some(1_700_000_000 - 40 + 100));
    }

    #[test]
    fn timestamp_uses_observed_at_not_send_time() {
        // Позиция замерена 5 c назад (debounce/rate-limit задержали
        // отправку): старт отсчитывается от замера, Discord не отстаёт.
        let settings = AppSettings::default();
        let mut playback = playing("1", "Track", &["Artist"], 40_000, 100_000);
        playback.observed_at = now() - std::time::Duration::from_secs(5);
        let activity = map_presence(&playback, &settings).unwrap();
        assert_eq!(activity.start_timestamp, Some(1_700_000_000 - 5 - 40));
        assert_eq!(activity.end_timestamp, Some(1_700_000_000 - 5 - 40 + 100));
    }

    #[test]
    fn paused_clears_timestamp_and_adds_marker() {
        let settings = AppSettings::default();
        let playback = playing("1", "Track", &["Artist"], 40_000, 100_000);
        let paused = PlaybackState {
            status: PlaybackStatus::Paused,
            ..playback
        };
        let activity = map_presence(&paused, &settings).unwrap();
        assert_eq!(activity.start_timestamp, None, "пауза — без таймстампа");
        assert_eq!(activity.end_timestamp, None, "пауза — без прогресс-бара");
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
        assert!(map_presence(&stopped, &settings).is_none());
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
        assert!(map_presence(&state, &paused_hidden).is_none());
        // Unknown при включённом показе — нейтральная activity без details.
        let activity = map_presence(&state, &paused_visible).unwrap();
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
        assert!(map_presence(&paused, &settings).is_none());
    }

    #[test]
    fn control_chars_removed() {
        let settings = AppSettings::default();
        let playback = playing("1", "Tra\x0cck", &["Artist"], 0, 100);
        let activity = map_presence(&playback, &settings).unwrap();
        assert_eq!(activity.details.as_deref(), Some("Track"));
    }

    #[test]
    fn https_only_button() {
        let settings = AppSettings::default();
        let mut playback = playing("1", "Track", &["Artist"], 0, 100);
        playback.track_url =
            Some(url::Url::parse("https://music.yandex.ru/album/1/track/2").unwrap());
        let activity = map_presence(&playback, &settings).unwrap();
        assert_eq!(
            activity.button_url.as_deref(),
            Some("https://music.yandex.ru/album/1/track/2")
        );

        playback.track_url = Some(url::Url::parse("http://music.yandex.ru/track/3").unwrap());
        let activity = map_presence(&playback, &settings).unwrap();
        assert!(activity.button_url.is_none(), "http не допускается");
    }

    #[test]
    fn long_titles_truncated_safely() {
        let settings = AppSettings::default();
        let long_title = "A".repeat(200);
        let playback = playing("1", &long_title, &["Artist"], 0, 100);
        let activity = map_presence(&playback, &settings).unwrap();
        assert!(activity.details.unwrap().chars().count() <= MAX_FIELD_LEN);
    }

    #[test]
    fn state_shows_only_artists_album_goes_to_large_text() {
        // Вторая строка — только автор, альбом — третья строка (large_text).
        let settings = AppSettings::default();
        let mut playback = playing("1", "Track", &["A", "B"], 0, 100);
        playback.album = Some("Album".into());
        let activity = map_presence(&playback, &settings).unwrap();
        assert_eq!(activity.details.as_deref(), Some("Track"));
        assert_eq!(activity.state.as_deref(), Some("A, B"));
        assert_eq!(activity.large_text.as_deref(), Some("Album"));
    }

    #[test]
    fn missing_values_leave_no_state() {
        // Нет артистов и альбома: вторая и третья строки пустые.
        let settings = AppSettings::default();
        let playback = playing("1", "Track", &[], 0, 100);
        let activity = map_presence(&playback, &settings).unwrap();
        assert_eq!(activity.details.as_deref(), Some("Track"));
        assert_eq!(activity.state, None);
        assert_eq!(activity.large_text, None);

        // Есть только альбом: вторая строка пуста, альбом — в третьей.
        let mut only_album = playing("1", "Track", &[], 0, 100);
        only_album.album = Some("Album".into());
        let activity = map_presence(&only_album, &settings).unwrap();
        assert_eq!(activity.state, None);
        assert_eq!(activity.large_text.as_deref(), Some("Album"));
    }

    #[test]
    fn button_label_matches_source_honestly() {
        use crate::sources::TrackSource;
        let settings = AppSettings::default();
        let mut playback = playing("1", "Track", &["Artist"], 0, 100);

        playback.track_url =
            Some(url::Url::parse("https://music.yandex.ru/album/1/track/2").unwrap());
        playback.track_source = Some(TrackSource::Yandex);
        assert_eq!(
            map_presence(&playback, &settings)
                .unwrap()
                .button_label
                .as_deref(),
            Some("Яндекс Музыка")
        );

        playback.track_url = Some(url::Url::parse("https://music.apple.com/us/song/1").unwrap());
        playback.track_source = Some(TrackSource::ITunes);
        let activity = map_presence(&playback, &settings).unwrap();
        assert_eq!(activity.button_label.as_deref(), Some("Открыть в iTunes"));
        assert_eq!(
            activity.button_url.as_deref(),
            Some("https://music.apple.com/us/song/1")
        );

        playback.track_url = Some(url::Url::parse("https://www.deezer.com/track/1").unwrap());
        playback.track_source = Some(TrackSource::Deezer);
        assert_eq!(
            map_presence(&playback, &settings)
                .unwrap()
                .button_label
                .as_deref(),
            Some("Открыть в Deezer")
        );

        // Legacy без источника — дефолт Яндекс (обратная совместимость).
        playback.track_source = None;
        assert_eq!(
            map_presence(&playback, &settings)
                .unwrap()
                .button_label
                .as_deref(),
            Some("Яндекс Музыка")
        );
    }

    #[test]
    fn button_labels_fit_discord_limits_in_chars_and_bytes() {
        // Регрессия: «Открыть в Яндекс Музыке» — 23 символа, но 43 байта;
        // такая подпись гасила всю активность для других пользователей
        // (лимит `MAX_BUTTON_LABEL_LEN`, видимо, считается в байтах —
        // сокращённые подписи FozerG «Откр. в браузере» влезают и так, и так).
        let limit = crate::discord::models::MAX_BUTTON_LABEL_LEN;
        for label in [
            crate::discord::models::OPEN_IN_YANDEX_LABEL,
            crate::discord::models::OPEN_IN_ITUNES_LABEL,
            crate::discord::models::OPEN_IN_DEEZER_LABEL,
        ] {
            assert!(
                label.chars().count() <= limit,
                "label слишком длинный: {label}"
            );
            assert!(
                label.len() <= limit,
                "label слишком длинный в байтах: {label}"
            );
        }
    }

    #[test]
    fn button_hidden_by_setting() {
        let settings = AppSettings {
            show_button: false,
            ..AppSettings::default()
        };
        let mut playback = playing("1", "Track", &["Artist"], 0, 100);
        playback.track_url = Some(url::Url::parse("https://music.yandex.ru/track/2").unwrap());
        let activity = map_presence(&playback, &settings).unwrap();
        assert!(activity.button_url.is_none());
        assert!(activity.button_label.is_none());
    }

    use crate::discord::models::MAX_FIELD_LEN;
}
