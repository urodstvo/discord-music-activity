//! Строгая проверка совпадений SMTC-снапшота с кандидатами каталогов.
//!
//! Прямой порт `ymrpc/matching.py` из FozerG/WinYandexMusicRPC (MIT):
//! `normalize`, `artists`, `title_parts`, `base_title`, `compare`, `choose`.
//! Семантика сверена с оригиналом построчно; отклонения зафиксированы ниже.
//!
//! Правила:
//! - title обязан совпасть (полностью или по базе без суффиксов в скобках);
//!   при совпадении только базы длительность обязательна для проверки;
//! - артисты SMTC должны быть подмножеством артистов кандидата
//!   (мультиартисты покрываются, `feat.`-гости входят в множество);
//! - длительность: `|a-b| <= max(8с, 4%)`, иначе отказ;
//! - выбор среди прошедших: совпавший альбом → ближайшая длительность →
//!   наличие обложки.
//!
//! Отклонение от Python: `casefold` заменён на `to_lowercase` (для кириллицы
//! и латиницы эквивалентно; расхождения возможны лишь на экзотике вида
//! турецкого `İ`). NFKC — через `unicode-normalization`, порядок операций
//! как в оригинале.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

static FEATURE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s*[\[(](?:feat\.?|ft\.?|featuring)\s+(.+?)[\)\]]").expect("valid FEATURE_RE")
});
static TRAILING_FEATURE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s+(?:feat\.?|ft\.?|featuring)\s+(.+)$").expect("valid TRAILING_FEATURE_RE")
});
static SEPARATOR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s*(?:,|;|&|\bfeat\.?\s|\bft\.?\s|\bfeaturing\s)\s*")
        .expect("valid SEPARATOR_RE")
});
static TRAILING_BRACKET_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\s*[\[(][^()\[\]]+[\)\]]\s*$").expect("valid TRAILING_BRACKET_RE")
});
static VERSION_SPLIT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+[–—-]\s+").expect("valid VERSION_SPLIT_RE"));

/// Минимальный интерфейс трека-кандидата для `compare`/`choose`.
/// Реализуется в `super::catalogs::TrackCandidate`.
pub trait MatchTrack {
    fn title(&self) -> &str;
    fn version(&self) -> &str;
    fn full_title(&self) -> String;
    fn artist(&self) -> &str;
    fn album(&self) -> &str;
    fn duration_secs(&self) -> f64;
    fn has_cover(&self) -> bool;
}

/// Минимальный интерфейс SMTC-снапшота для `compare`/`choose`.
pub trait MatchMedia {
    fn title(&self) -> &str;
    fn artist(&self) -> &str;
    fn album(&self) -> &str;
    fn duration_secs(&self) -> f64;
}

/// NFKC + lowercase + `ё→е`, всё не-алфанумерик → пробел, схлопнуть пробелы.
pub fn normalize(value: &str) -> String {
    let nfkc: String = value.nfkc().collect();
    let lower = nfkc.to_lowercase().replace('ё', "е");
    let mut out = String::with_capacity(lower.len());
    for c in lower.chars() {
        if c.is_alphanumeric() {
            out.push(c);
        } else {
            out.push(' ');
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Множество нормализованных артистов (split по `,; PushButton&` + feat-ключи).
pub fn artists(value: &str) -> HashSet<String> {
    SEPARATOR_RE
        .split(value)
        .map(normalize)
        .filter(|s| !s.is_empty())
        .collect()
}

/// Нормализованный title + множество feat-гостей из скобок и хвоста.
pub fn title_parts(value: &str) -> (String, HashSet<String>) {
    let mut guests: HashSet<String> = HashSet::new();
    for cap in FEATURE_RE.captures_iter(value) {
        if let Some(group) = cap.get(1) {
            guests.extend(artists(group.as_str()));
        }
    }
    let mut stripped = FEATURE_RE.replace_all(value, "").to_string();
    if let Some(cap) = TRAILING_FEATURE_RE.captures(&stripped.clone()) {
        if let Some(group) = cap.get(1) {
            guests.extend(artists(group.as_str()));
        }
        stripped = TRAILING_FEATURE_RE.replace(&stripped, "").to_string();
    }
    (normalize(&stripped), guests)
}

/// Базовый title без feat, трейлинг-скобок и версии после ` — `.
pub fn base_title(value: &str) -> String {
    let mut current = FEATURE_RE.replace_all(value, "").to_string();
    current = TRAILING_FEATURE_RE.replace_all(&current, "").to_string();
    loop {
        let shortened = TRAILING_BRACKET_RE.replace(&current, "").to_string();
        if shortened == current || shortened.trim().is_empty() {
            break;
        }
        current = shortened;
    }
    let first = VERSION_SPLIT_RE.split(&current).next().unwrap_or(&current);
    normalize(first)
}

/// Строгая проверка пары (SMTC, кандидат). Возвращает причину отказа/приёма
/// (как в оригинале — для логов).
pub fn compare<M: MatchMedia, T: MatchTrack>(media: &M, track: &T) -> (bool, &'static str) {
    let (title, guests) = title_parts(media.title());
    let (candidate, candidate_guests) = title_parts(track.title());
    let (full_title, version_guests) = title_parts(&track.full_title());
    let mut actual = candidate_guests;
    actual.extend(version_guests);
    let different_suffix = title != candidate && title != full_title;
    if title.is_empty() {
        return (false, "missing title");
    }
    if different_suffix {
        let base = base_title(media.title());
        if base.is_empty() || base != base_title(track.title()) {
            return (false, "base title differs");
        }
        if media.duration_secs() <= 0.0 || track.duration_secs() <= 0.0 {
            return (
                false,
                "different title suffixes require duration verification",
            );
        }
    }
    let mut wanted = artists(media.artist());
    wanted.extend(guests);
    let mut have = artists(track.artist());
    have.extend(actual);
    if wanted.is_empty() || !wanted.is_subset(&have) {
        return (false, "artist credits differ or are missing");
    }
    if media.duration_secs() > 0.0 && track.duration_secs() > 0.0 {
        let diff = (media.duration_secs() - track.duration_secs()).abs();
        let tolerance = (media.duration_secs() * 0.04).max(8.0);
        if diff > tolerance {
            return (false, "duration differs");
        }
    }
    if different_suffix {
        return (
            true,
            "base title, artist credits and duration agree; title suffixes differ",
        );
    }
    if !track.version().is_empty() && title == candidate && title != full_title {
        return (
            true,
            "title, artist credits and available duration agree; player omits catalog version",
        );
    }
    (true, "title, artist credits and available duration agree")
}

/// Выбор лучшего из прошедших проверку: альбом → длительность → обложка.
pub fn choose<'a, M: MatchMedia, T: MatchTrack>(media: &M, tracks: &'a [T]) -> Option<&'a T> {
    let mut candidates: Vec<&'a T> = tracks.iter().filter(|t| compare(media, *t).0).collect();
    if candidates.is_empty() {
        return None;
    }
    candidates.sort_by(|a, b| {
        let album_a = !media.album().is_empty() && normalize(media.album()) != normalize(a.album());
        let album_b = !media.album().is_empty() && normalize(media.album()) != normalize(b.album());
        album_a
            .cmp(&album_b)
            .then_with(|| {
                let da = if media.duration_secs() > 0.0 && a.duration_secs() > 0.0 {
                    (media.duration_secs() - a.duration_secs()).abs()
                } else {
                    999.0
                };
                let db = if media.duration_secs() > 0.0 && b.duration_secs() > 0.0 {
                    (media.duration_secs() - b.duration_secs()).abs()
                } else {
                    999.0
                };
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| (!a.has_cover()).cmp(&(!b.has_cover())))
    });
    candidates.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Media {
        title: String,
        artist: String,
        album: String,
        duration: f64,
    }

    impl MatchMedia for Media {
        fn title(&self) -> &str {
            &self.title
        }
        fn artist(&self) -> &str {
            &self.artist
        }
        fn album(&self) -> &str {
            &self.album
        }
        fn duration_secs(&self) -> f64 {
            self.duration
        }
    }

    struct Track {
        title: String,
        version: String,
        artist: String,
        album: String,
        duration: f64,
        cover: bool,
    }

    impl MatchTrack for Track {
        fn title(&self) -> &str {
            &self.title
        }
        fn version(&self) -> &str {
            &self.version
        }
        fn full_title(&self) -> String {
            if self.version.is_empty() {
                self.title.clone()
            } else {
                format!("{} ({})", self.title, self.version)
            }
        }
        fn artist(&self) -> &str {
            &self.artist
        }
        fn album(&self) -> &str {
            &self.album
        }
        fn duration_secs(&self) -> f64 {
            self.duration
        }
        fn has_cover(&self) -> bool {
            self.cover
        }
    }

    fn media(title: &str, artist: &str, album: &str, duration: f64) -> Media {
        Media {
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            duration,
        }
    }

    fn track(title: &str, artist: &str, album: &str, duration: f64) -> Track {
        Track {
            title: title.into(),
            version: String::new(),
            artist: artist.into(),
            album: album.into(),
            duration,
            cover: true,
        }
    }

    #[test]
    fn normalize_collapses_and_yo() {
        assert_eq!(normalize("  Ёлка,  ПРИВЕТ! "), "елка привет");
        assert_eq!(normalize(""), "");
    }

    #[test]
    fn exact_match_passes() {
        let m = media("Track", "Artist", "Album", 200.0);
        let t = track("Track", "Artist", "Album", 200.0);
        assert!(compare(&m, &t).0);
    }

    #[test]
    fn missing_title_fails() {
        let m = media("", "Artist", "", 200.0);
        let t = track("Track", "Artist", "", 200.0);
        assert!(!compare(&m, &t).0);
    }

    #[test]
    fn missing_artist_fails() {
        let m = media("Track", "", "", 200.0);
        let t = track("Track", "Artist", "", 200.0);
        assert!(!compare(&m, &t).0);
    }

    #[test]
    fn duration_tolerance() {
        let m = media("Track", "Artist", "", 200.0);
        assert!(compare(&m, &track("Track", "Artist", "", 205.0)).0);
        assert!(!compare(&m, &track("Track", "Artist", "", 250.0)).0);
    }

    #[test]
    fn feat_guest_counts_as_artist() {
        let m = media("Track (feat. Guest)", "Artist", "", 200.0);
        let t = track("Track", "Artist, Guest", "", 200.0);
        assert!(compare(&m, &t).0, "гость из скобок входит во множество");
    }

    #[test]
    fn remaster_suffix_needs_duration() {
        let m = media("Track", "Artist", "", 200.0);
        let t = track("Track (Remaster)", "Artist", "", 200.0);
        assert!(compare(&m, &t).0);
        let nodur = media("Track", "Artist", "", 0.0);
        assert!(!compare(&nodur, &t).0);
    }

    #[test]
    fn artist_subset_covers_multiartist() {
        let m = media("Track", "Artist1", "", 200.0);
        let t = track("Track", "Artist1, Artist2", "", 200.0);
        assert!(compare(&m, &t).0);
        let m2 = media("Track", "Artist1, Artist2", "", 200.0);
        let t2 = track("Track", "Artist1", "", 200.0);
        assert!(!compare(&m2, &t2).0);
    }

    #[test]
    fn choose_prefers_album_then_duration_then_cover() {
        let m = media("Track", "Artist", "Album", 200.0);
        let wrong_album = track("Track", "Artist", "Other", 200.0);
        let no_cover = Track {
            cover: false,
            ..track("Track", "Artist", "Album", 200.0)
        };
        let with_cover = Track {
            cover: true,
            ..track("Track", "Artist", "Album", 200.0)
        };
        // Тот же альбом и та же длительность — решает обложка.
        let list = vec![wrong_album, no_cover, with_cover];
        let picked = choose(&m, &list).unwrap();
        assert_eq!(picked.album, "Album");
        assert!(picked.cover);
        // При равном альбоме и обложке — ближайшая длительность.
        let near = track("Track", "Artist", "Album", 202.0);
        let far = track("Track", "Artist", "Album", 205.0);
        let list2 = vec![far, near];
        assert_eq!(choose(&m, &list2).unwrap().duration, 202.0);
    }
}
