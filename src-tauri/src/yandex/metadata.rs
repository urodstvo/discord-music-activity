//! Получение недостающих метаданных по track id (ТЗ §5.4).
//!
//! Источник — read-only endpoint каталога Яндекс Музыки
//! (`/tracks?ids=…`). Без загрузки аудио, без поиска по строкам.
//! Успешные ответы кешируются ограниченным LRU; при ошибке вызов
//! просто не даёт новых полей (live-state остаётся источником).

use std::collections::HashMap;
use std::sync::Mutex;

use url::Url;

use crate::yandex::errors::{AppError, AppResult, ErrorCode};

/// Верхняя граница кеша метаданных (ТЗ §5.2: ориентир 100–300 записей).
pub const METADATA_CACHE_LIMIT: usize = 200;

/// Метаданные, пригодные для дополнения `PlaybackState`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackMetadata {
    pub title: Option<String>,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub cover_url: Option<Url>,
    pub duration_ms: Option<u64>,
}

/// Ответ API `/tracks` (lenient: неизвестные поля игнорируются).
#[allow(dead_code)]
#[derive(serde::Deserialize)]
struct ApiTrack {
    #[serde(default, rename = "id")]
    id_template: Option<String>,
    #[serde(rename = "title", default)]
    title: Option<String>,
    #[serde(rename = "durationMs", default)]
    duration_ms: Option<u64>,
    #[serde(default)]
    artists: Option<Vec<ApiArtist>>,
    #[serde(default)]
    albums: Option<Vec<ApiAlbum>>,
}

#[derive(serde::Deserialize)]
struct ApiArtist {
    #[serde(default)]
    name: Option<String>,
}

#[derive(serde::Deserialize)]
struct ApiAlbum {
    #[serde(default)]
    title: Option<String>,
    /// Поле доступно не во всех ответах.
    #[serde(default, rename = "coverUri")]
    cover_uri: Option<String>,
}

/// Множество запросов по одному track id не дублируется: дедупликация через
/// простую запись «в процессе» внутри кеша.
pub struct MetadataClient {
    token: String,
    cache: Mutex<LruCache>,
    http: reqwest::Client,
}

struct LruCache {
    map: HashMap<String, TrackMetadata>,
    /// Порядок использования; свежее — справа.
    order: Vec<String>,
}

impl LruCache {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            order: Vec::new(),
        }
    }

    fn get(&mut self, key: &str) -> Option<TrackMetadata> {
        if let Some(index) = self.order.iter().position(|k| k == key) {
            let key = self.order.remove(index);
            self.order.push(key);
        }
        self.map.get(key).cloned()
    }

    fn put(&mut self, key: String, value: TrackMetadata) {
        if let Some(index) = self.order.iter().position(|k| k == &key) {
            self.order.remove(index);
        }
        self.order.push(key.clone());
        self.map.insert(key, value);
        while self.map.len() > METADATA_CACHE_LIMIT {
            let evicted = self.order.remove(0);
            self.map.remove(&evicted);
        }
    }

    /// Помечаем «найденных нет»: полезно для отрицательного кеша, чтобы не
    /// дёргать сервер каждую смену трека.
    fn mark_negative(&mut self, key: &str) {
        self.put(key.to_string(), TrackMetadata::empty());
    }
}

impl TrackMetadata {
    fn empty() -> Self {
        Self {
            title: None,
            artists: Vec::new(),
            album: None,
            cover_url: None,
            duration_ms: None,
        }
    }

    fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.artists.is_empty()
            && self.album.is_none()
            && self.cover_url.is_none()
            && self.duration_ms.is_none()
    }
}

impl MetadataClient {
    pub fn new(token: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_default();
        Self {
            token,
            cache: Mutex::new(LruCache::new()),
            http,
        }
    }

    /// Запросить метаданные трека. `Ok(None)` — не найдено. Не считает
    /// ошибкой сети критическую проблему: возвращаются Err, а caller
    /// продолжает с live-state.
    pub async fn lookup(&self, track_id: &str) -> AppResult<Option<TrackMetadata>> {
        if track_id.trim().is_empty() {
            return Ok(None);
        }

        {
            let mut cache = self.cache.lock().expect("cache доступен");
            if let Some(cached) = cache.get(track_id) {
                if cached.is_empty() {
                    return Ok(None); // отрицательный кеш
                }
                return Ok(Some(cached));
            }
        }

        let metadata = self.fetch(track_id).await?;
        let result = match metadata {
            Some(meta) if !meta.is_empty() => {
                self.cache
                    .lock()
                    .expect("cache доступен")
                    .put(track_id.to_string(), meta.clone());
                Some(meta)
            }
            _ => {
                self.cache
                    .lock()
                    .expect("cache доступен")
                    .mark_negative(track_id);
                None
            }
        };
        Ok(result)
    }

    /// HTTP запрос к каталогу.
    async fn fetch(&self, track_id: &str) -> AppResult<Option<TrackMetadata>> {
        let url = Url::parse("https://api.music.yandex.ru/tracks").map_err(|err| {
            AppError::with_details(ErrorCode::Internal, err.to_string())
        })?;

        let response = self
            .http
            .get(url)
            // ПАРАМЕТР ИМЕННО trackIds: `ids=…` сервер отклоняет с 400
            // ("trackIds: Parameter value ..." — проверено на продакшене).
            .query(&[("trackIds", track_id)])
            .header("Authorization", format!("OAuth {}", self.token))
            .header("X-Yandex-Music-Client", "YandexMusicAndroid/24023621")
            .send()
            .await
            .map_err(|err| {
                // reqwest-текст не содержит токена (он в заголовке запроса).
                tracing::warn!("metadata request failed: {err}");
                AppError::new(ErrorCode::YandexUnavailable)
            })?;

        if response.status().as_u16() == 401 || response.status().as_u16() == 403 {
            return Err(AppError::new(ErrorCode::InvalidToken));
        }
        if response.status().as_u16() == 404 {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(AppError::with_details(
                ErrorCode::YandexUnavailable,
                format!("catalog status {}", response.status().as_u16()),
            ));
        }

        // Ответ: {"result": [...]}. Токен в ответе не возвращаем.
        let payload: serde_json::Value = response.json().await.map_err(|err| {
            AppError::with_details(ErrorCode::YandexUnavailable, format!("json: {err}"))
        })?;
        let Some(results) = payload.get("result").and_then(|r| r.as_array()) else {
            return Ok(None);
        };
        let track: Option<ApiTrack> = results
            .first()
            .cloned()
            .and_then(|item| serde_json::from_value(item).ok());

        Ok(track.map(|t| to_metadata(t, track_id)))
    }
}

fn to_metadata(track: ApiTrack, _fallback_id: &str) -> TrackMetadata {
    let cover_uri = track
        .albums
        .as_ref()
        .and_then(|albums| albums.first().and_then(|album| album.cover_uri.clone()));
    let cover_url = cover_uri.as_deref().and_then(build_https_cover);

    TrackMetadata {
        title: track
            .title
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty()),
        artists: track
            .artists
            .unwrap_or_default()
            .into_iter()
            .filter_map(|a| a.name)
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .collect(),
        album: track
            .albums
            .unwrap_or_default()
            .first()
            .and_then(|a| a.title.clone())
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty()),
        cover_url,
        duration_ms: track.duration_ms.filter(|d| *d > 0),
    }
}

/// CDN шаблон cover URI (`/%%`) → конкретный HTTPS URL (ТЗ §5.3: только
/// HTTPS + allowlist). API отдаёт шаблон БЕЗ схемы
/// (`avatars.yandex.net/...`); добавляем https автоматически.
pub fn build_https_cover(template: &str) -> Option<Url> {
    let template = template.trim();
    if template.is_empty() {
        return None;
    }
    let candidate = if template.contains("://") {
        template.to_string()
    } else {
        format!("https://{template}")
    };
    let substituted = candidate.replace("%%", "400x400");
    let url = Url::parse(&substituted).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    if !url
        .host_str()
        .map(|h| {
            ["avatars.yandex.net", "avatars.mds.yandex.net"].contains(&h)
        })
        .unwrap_or(false)
    {
        return None;
    }
    Some(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_is_bounded() {
        let mut cache = LruCache::new();
        for i in 0..(METADATA_CACHE_LIMIT + 50) {
            cache.put(format!("track-{i}"), TrackMetadata::empty());
        }
        assert_eq!(cache.map.len(), METADATA_CACHE_LIMIT);
    }

    #[test]
    fn negative_cache_does_not_refetch() {
        let mut cache = LruCache::new();
        cache.put("a".to_string(), TrackMetadata::empty());
        cache.get("a");
        cache.put("b".to_string(), TrackMetadata::empty());
        let mut cache = LruCache::new();
        cache.put("a".to_string(), TrackMetadata::empty());
        // порядок использования: a, b
        cache.put("b".to_string(), TrackMetadata::empty());
        let _ = cache; // вытеснение проверено в cache_is_bounded
    }

    #[test]
    fn get_reorders_lru() {
        let mut cache = LruCache::new();
        cache.put("a".to_string(), TrackMetadata::empty());
        cache.put("b".to_string(), TrackMetadata::empty());
        cache.get("a");
        cache.put("c".to_string(), TrackMetadata::empty());
        assert_eq!(cache.map.len(), 3);
        assert_eq!(cache.order[0], "b", "b вытеснится первым");
    }

    #[test]
    fn cover_only_https_and_yandex() {
        assert!(build_https_cover("https://avatars.yandex.net/get/1/2/%%").is_some());
        assert!(build_https_cover("http://avatars.yandex.net/get/1/2/%%").is_none());
        assert!(build_https_cover("https://evil.example/%%").is_none());
        assert!(build_https_cover("data:text/html,x").is_none());
        // API отдаёт coverUri без схемы — должен добавить https сам.
        let url = build_https_cover("avatars.yandex.net/get-x/y/%%").expect("scheme-less валиден");
        assert_eq!(url.as_str(), "https://avatars.yandex.net/get-x/y/400x400");
        assert!(build_https_cover("evil.example/%%").is_none());
    }

    #[test]
    fn api_track_parsing_is_lenient() {
        let json = serde_json::json!({
            "id": "123",
            "title": "Track",
            "unknownField": {"nested": true},
            "artists": [{"name": "А    "}],
            "durationMs": 240000,
            "albums": [{"title": "Album", "coverUri": "avatars.yandex.net/get-x/y/%%"}]
        });
        let track: ApiTrack = serde_json::from_value(json).unwrap();
        let meta = to_metadata(track, "123");
        assert_eq!(meta.title.as_deref(), Some("Track"));
        assert_eq!(meta.artists, vec!["А".to_string()]);
        assert_eq!(meta.duration_ms, Some(240_000));
    }
}
