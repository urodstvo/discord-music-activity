//! Фоновое обогащение SMTC-состояний данными каталогов.
//!
//! Поток данных (исправление «сброса при перемотке»):
//! live-состояние публикуется сразу; поиск идёт отдельным tokio-таском и
//! возвращает результат в цикл опроса через локальный канал, а не напрямую
//! в `SourceEvent`. Цикл хранит короткоживущий кеш «трек → обогащение»
//! (cap 32) и подкладывает его в КАЖДОЕ публикуемое состояние — и на
//! `Identity`, и на подтверждённый `Seek`. Поэтому перемотка в пределах
//! трека картинку/кнопку больше не сбрасывает, а позиции из протухших
//! fetch-результатов никого не перезаписывают (merge идёт на свежий замер).

use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

use super::catalogs::{EnrichedTrack, SmtcQuery, TrackResolver};
use crate::sources::PlaybackState;

/// Ключ идентичности трека для кеша (совпадает с триггером Identity:
/// смена любого поля — новый трек, перемотка ключ не меняет).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdentityKey {
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
}

impl IdentityKey {
    pub fn of(state: &PlaybackState) -> Option<Self> {
        let title = state.title.as_deref()?.trim();
        if title.is_empty() || state.artists.is_empty() {
            return None;
        }
        Some(Self {
            title: title.to_string(),
            artists: state.artists.clone(),
            album: state.album.clone(),
            duration_ms: state.duration_ms,
        })
    }
}

/// Результат фонового поиска для цикла опроса.
#[derive(Debug)]
pub struct EnrichmentResult {
    pub key: IdentityKey,
    pub enriched: EnrichedTrack,
}

/// Короткоживущий кеш «трек → обогащение» (cap 32, FIFO).
/// Живёт в цикле опроса; `TrackResolver` дополнительно держит свой
/// сетевой кеш с TTL (3600с hit / 60с miss).
#[derive(Debug, Default)]
pub struct EnrichmentCache {
    map: HashMap<IdentityKey, EnrichedTrack>,
    order: Vec<IdentityKey>,
}

impl EnrichmentCache {
    pub const LIMIT: usize = 32;

    pub fn get(&self, key: &IdentityKey) -> Option<&EnrichedTrack> {
        self.map.get(key)
    }

    pub fn insert(&mut self, key: IdentityKey, value: EnrichedTrack) {
        if let Some(pos) = self.order.iter().position(|k| k == &key) {
            self.order.remove(pos);
        }
        self.order.push(key.clone());
        self.map.insert(key, value);
        while self.map.len() > Self::LIMIT {
            let oldest = self.order.remove(0);
            self.map.remove(&oldest);
        }
    }
}

/// Общий резолвер + счётчик поколений для отмены устаревших поисков.
#[derive(Debug)]
pub struct EnrichmentShared {
    pub resolver: Mutex<TrackResolver>,
    pub generation: AtomicU64,
}

impl EnrichmentShared {
    pub fn new() -> Self {
        Self {
            resolver: Mutex::new(TrackResolver::new()),
            generation: AtomicU64::new(0),
        }
    }

    /// Поднять поколение: все фоновые поиски предыдущего трека устаревают
    /// (сессии кончились, источник ошибится или трек сменился).
    pub fn bump_generation(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
}

/// Построить запрос каталогам из live-состояния. `None` — искать нечего
/// (нет title или нет артистов: `compare` всё равно откажет).
pub fn build_query(state: &PlaybackState) -> Option<SmtcQuery> {
    let title = state.title.as_deref()?.trim();
    if title.is_empty() || state.artists.is_empty() {
        return None;
    }
    Some(SmtcQuery {
        title: title.to_string(),
        artist: state.artists.join(", "),
        album: state.album.clone().unwrap_or_default(),
        duration_secs: state
            .duration_ms
            .map(|ms| ms as f64 / 1000.0)
            .unwrap_or(0.0),
    })
}

/// Подложить кешированное обогащение в свежее состояние: player-правда
/// (title/artists/позиция/`observed_at`) не трогается, добавляются только
/// обложка/ссылка/источник + недостающие album/duration.
/// Возвращает `true`, если состояние изменилось.
pub fn attach_cached(state: &mut PlaybackState, cache: &EnrichmentCache) -> bool {
    let Some(key) = IdentityKey::of(state) else {
        return false;
    };
    let Some(enriched) = cache.get(&key) else {
        tracing::debug!(
            "SMTC enrichment: кеш-промах для {:?} / {:?}",
            state.title,
            state.artists,
        );
        return false;
    };
    let mut changed = false;
    if state.cover_url.is_none() && enriched.cover_url.is_some() {
        state.cover_url = enriched.cover_url.clone();
        changed = true;
    }
    if state.track_url.is_none() && enriched.track_url.is_some() {
        state.track_url = enriched.track_url.clone();
        state.track_source = Some(enriched.source);
        changed = true;
    }
    if state.album.is_none() && enriched.album.is_some() {
        state.album = enriched.album.clone();
        changed = true;
    }
    if state.duration_ms.is_none() && enriched.duration_ms.is_some() {
        state.duration_ms = enriched.duration_ms;
        changed = true;
    }
    if changed {
        tracing::debug!(
            "SMTC enrichment: кеш-хит ({:?}) — cover: {}, url: {}",
            enriched.source,
            state.cover_url.is_some(),
            state.track_url.is_some(),
        );
    }
    changed
}

/// Запустить фоновый поиск для свежего Identity-состояния.
/// Результат вернётся в цикл опроса через `result_tx` (не в `SourceEvent`!),
/// цикл подложит его в свежий замер — протухшая позиция никого не затрёт.
pub fn spawn_fetch(
    shared: Arc<EnrichmentShared>,
    shutdown: CancellationToken,
    result_tx: mpsc::UnboundedSender<EnrichmentResult>,
    query: SmtcQuery,
    key: IdentityKey,
    token: Option<String>,
) {
    shared.bump_generation();
    let generation = shared.generation.load(Ordering::SeqCst);
    tracing::debug!(
        "SMTC enrichment: ищу {:?} / {:?} (токен: {})",
        query.title,
        query.artist,
        if token.is_some() { "да" } else { "нет" },
    );
    tokio::spawn(async move {
        if shutdown.is_cancelled() {
            return;
        }
        let enriched = {
            let mut resolver = shared.resolver.lock().await;
            resolver.resolve(&query, token.as_deref()).await
        };
        if shutdown.is_cancelled() {
            return;
        }
        // Трек уже переключился — результат устарел, молча выкидываем.
        if shared.generation.load(Ordering::SeqCst) != generation {
            tracing::debug!("SMTC enrichment: трек сменился, результат отброшен");
            return;
        }
        let Some(enriched) = enriched else {
            tracing::debug!(
                "SMTC enrichment: ничего не найдено для {:?} / {:?}",
                query.title,
                query.artist,
            );
            return;
        };
        tracing::info!(
            "SMTC enrichment: хит {:?} — {} (cover: {}, url: {})",
            enriched.source,
            enriched.title,
            enriched.cover_url.is_some(),
            enriched
                .track_url
                .as_ref()
                .map(|u| u.host_str().unwrap_or("?").to_string())
                .unwrap_or_else(|| "none".to_string()),
        );
        let _ = result_tx.send(EnrichmentResult { key, enriched });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::PlaybackStatus;
    use std::time::SystemTime;
    use url::Url;

    fn live() -> PlaybackState {
        PlaybackState {
            status: PlaybackStatus::Playing,
            track_id: None,
            title: Some("Track".into()),
            artists: vec!["Artist".into()],
            album: None,
            duration_ms: None,
            position_ms: Some(1_000),
            cover_url: None,
            track_url: None,
            track_source: None,
            source_device: Some("App.exe".into()),
            observed_at: SystemTime::now(),
        }
    }

    fn enriched() -> EnrichedTrack {
        EnrichedTrack {
            title: "Catalog Title".into(),
            artist: "Catalog Artist".into(),
            album: Some("Catalog Album".into()),
            duration_ms: Some(200_000),
            cover_url: Some(Url::parse("https://is1-ssl.mzstatic.com/a/600x600bb.jpg").unwrap()),
            track_url: Some(Url::parse("https://music.apple.com/us/song/1").unwrap()),
            source: crate::sources::TrackSource::ITunes,
        }
    }

    #[test]
    fn build_query_needs_title_and_artists() {
        assert!(build_query(&live()).is_some());
        let mut no_artist = live();
        no_artist.artists.clear();
        assert!(build_query(&no_artist).is_none());
        let mut no_title = live();
        no_title.title = None;
        assert!(build_query(&no_title).is_none());
    }

    #[test]
    fn identity_key_matches_identity_trigger() {
        let a = IdentityKey::of(&live()).unwrap();
        // Перемотка (позиция) ключ не меняет.
        let mut moved = live();
        moved.position_ms = Some(99_000);
        assert_eq!(IdentityKey::of(&moved).unwrap(), a);
        // Смена названия — новый ключ.
        let mut renamed = live();
        renamed.title = Some("Other".into());
        assert_ne!(IdentityKey::of(&renamed).unwrap(), a);
    }

    #[test]
    fn attach_keeps_player_truth_and_observed_at() {
        let before = live();
        let at = before.observed_at;
        let mut state = before;
        let mut cache = EnrichmentCache::default();
        cache.insert(IdentityKey::of(&state).unwrap(), enriched());
        assert!(attach_cached(&mut state, &cache));
        assert_eq!(state.title.as_deref(), Some("Track"));
        assert_eq!(state.artists, vec!["Artist".to_string()]);
        assert_eq!(state.position_ms, Some(1_000));
        assert_eq!(state.observed_at, at, "замер не трогаем");
        assert_eq!(state.album.as_deref(), Some("Catalog Album"));
        assert_eq!(state.duration_ms, Some(200_000));
        assert_eq!(
            state.track_source,
            Some(crate::sources::TrackSource::ITunes)
        );
        // Повторный attach — без изменений.
        assert!(!attach_cached(&mut state, &cache));
    }

    #[test]
    fn attach_does_not_overwrite_live_cover() {
        let mut state = live();
        state.cover_url = Some(Url::parse("https://avatars.yandex.net/x/400x400").unwrap());
        let mut cache = EnrichmentCache::default();
        cache.insert(IdentityKey::of(&state).unwrap(), enriched());
        attach_cached(&mut state, &cache);
        assert_eq!(
            state.cover_url.as_ref().map(Url::as_str),
            Some("https://avatars.yandex.net/x/400x400")
        );
        // Кнопки не было — она добавилась.
        assert!(state.track_url.is_some());
    }

    #[test]
    fn attach_miss_on_unknown_track() {
        let mut state = live();
        let cache = EnrichmentCache::default();
        assert!(!attach_cached(&mut state, &cache));
        assert!(state.cover_url.is_none());
    }

    #[test]
    fn cache_is_bounded() {
        let mut cache = EnrichmentCache::default();
        for i in 0..(EnrichmentCache::LIMIT + 10) {
            let key = IdentityKey {
                title: format!("t{i}"),
                artists: vec!["a".into()],
                album: None,
                duration_ms: None,
            };
            cache.insert(key, enriched());
        }
        assert_eq!(cache.map.len(), EnrichmentCache::LIMIT);
    }
}
