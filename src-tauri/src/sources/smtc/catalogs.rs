//! Каталоги для обогащения SMTC: Yandex → iTunes → Deezer.
//!
//! Порт каскада `ymrpc/catalogs.py + yandex.py + search.py`
//! из FozerG/WinYandexMusicRPC (MIT). Порядок и лимиты сохранены:
//! iTunes throttled 18 запр/мин, cap ответа 2 МБ, таймаут 4 с на ступень.
//! Yandex идёт первым (с токеном, если есть, иначе анонимно); `401/403`
//! переводят ступень в `unavailable` с backoff 60 с, а не в ошибку всего поиска.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use serde::Deserialize;
use url::Url;

use super::matching::{MatchMedia, MatchTrack, choose, normalize};
use crate::sources::{PlaybackStatus, TrackSource};

/// Кандидат каталога (нормализованный трек + ссылки).
#[derive(Debug, Clone, PartialEq)]
pub struct TrackCandidate {
    pub title: String,
    pub version: String,
    pub artist: String,
    pub album: String,
    pub duration_secs: f64,
    pub cover_url: Option<Url>,
    pub page_url: Option<Url>,
    pub source: TrackSource,
}

impl TrackCandidate {
    pub fn full_title(&self) -> String {
        if self.version.is_empty() {
            self.title.clone()
        } else {
            format!("{} ({})", self.title, self.version)
        }
    }
}

impl MatchTrack for TrackCandidate {
    fn title(&self) -> &str {
        &self.title
    }
    fn version(&self) -> &str {
        &self.version
    }
    fn full_title(&self) -> String {
        self.full_title()
    }
    fn artist(&self) -> &str {
        &self.artist
    }
    fn album(&self) -> &str {
        &self.album
    }
    fn duration_secs(&self) -> f64 {
        self.duration_secs
    }
    fn has_cover(&self) -> bool {
        self.cover_url.is_some()
    }
}

/// SMTC-снапшот, адаптированный под `MatchMedia`.
#[derive(Debug, Clone)]
pub struct SmtcQuery {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_secs: f64,
}

impl MatchMedia for SmtcQuery {
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
        self.duration_secs
    }
}

/// Итог обогащения: готовые поля для `PlaybackState`.
#[derive(Debug, Clone, PartialEq)]
pub struct EnrichedTrack {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
    pub cover_url: Option<Url>,
    pub track_url: Option<Url>,
    pub source: TrackSource,
}

impl From<TrackCandidate> for EnrichedTrack {
    fn from(c: TrackCandidate) -> Self {
        Self {
            title: c.full_title(),
            artist: c.artist,
            album: (!c.album.is_empty()).then_some(c.album),
            duration_ms: (c.duration_secs > 0.0).then_some((c.duration_secs * 1000.0) as u64),
            cover_url: c.cover_url,
            track_url: c.page_url,
            source: c.source,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum CatalogKind {
    Yandex,
    ITunes,
    Deezer,
}

/// Состояние резолвера: кеш LRU 128 (hit 3600 с / miss 60 с),
/// backoff провайдера 60 с, throttle iTunes 18/мин.
#[derive(Debug)]
pub struct TrackResolver {
    http: reqwest::Client,
    cache: HashMap<String, (Instant, Instant, Option<EnrichedTrack>)>,
    order: Vec<String>,
    backoff: HashMap<CatalogKind, Instant>,
    itunes_requests: VecDeque<Instant>,
}

impl TrackResolver {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(4))
            .build()
            .unwrap_or_default();
        Self {
            http,
            cache: HashMap::new(),
            order: Vec::new(),
            backoff: HashMap::new(),
            itunes_requests: VecDeque::new(),
        }
    }

    fn cache_key(query: &SmtcQuery) -> String {
        format!(
            "{}\x1f{}\x1f{}\x1f{}",
            normalize(&query.title),
            normalize(&query.artist),
            normalize(&query.album),
            query.duration_secs.round() as i64
        )
    }

    fn cache_get(&mut self, key: &str, now: Instant) -> Option<Option<EnrichedTrack>> {
        let (expires_at, _, value) = self.cache.get(key)?;
        if now >= *expires_at {
            return None;
        }
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            let k = self.order.remove(pos);
            self.order.push(k);
        }
        Some(value.clone())
    }

    fn cache_put(&mut self, key: String, value: Option<EnrichedTrack>, now: Instant) {
        let ttl = if value.is_some() {
            Duration::from_secs(3600)
        } else {
            Duration::from_secs(60)
        };
        if let Some(pos) = self.order.iter().position(|k| k == &key) {
            self.order.remove(pos);
        }
        self.order.push(key.clone());
        self.cache.insert(key, (now, now + ttl, value));
        while self.cache.len() > 128 {
            let oldest = self.order.remove(0);
            self.cache.remove(&oldest);
        }
    }

    fn backoff_active(&self, kind: CatalogKind, now: Instant) -> bool {
        self.backoff.get(&kind).is_some_and(|until| now < *until)
    }

    fn mark_unavailable(&mut self, kind: CatalogKind, now: Instant) {
        self.backoff.insert(kind, now + Duration::from_secs(60));
    }

    fn itunes_budget_ok(&mut self, now: Instant) -> bool {
        while self
            .itunes_requests
            .front()
            .is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(60))
        {
            self.itunes_requests.pop_front();
        }
        if self.itunes_requests.len() >= 18 {
            return false;
        }
        self.itunes_requests.push_back(now);
        true
    }

    /// Полный каскад. `token` — `Some`, если в Credential Manager есть токен
    /// (режим auto/media_control с авторизацией); иначе анонимный Yandex.
    /// Возвращает первый строго проверенный (`choose`) хит.
    pub async fn resolve(
        &mut self,
        query: &SmtcQuery,
        token: Option<&str>,
    ) -> Option<EnrichedTrack> {
        if query.artist.trim().is_empty() || query.title.trim().is_empty() {
            tracing::debug!("SMTC resolve: пропуск — нет артиста или названия");
            return None;
        }
        let now = Instant::now();
        let key = Self::cache_key(query);
        if let Some(cached) = self.cache_get(&key, now) {
            tracing::debug!(
                "SMTC resolve: кеш-хит ({}) для {:?} / {:?}",
                cached
                    .as_ref()
                    .map(|c| format!("{:?}", c.source))
                    .unwrap_or_else(|| "miss".to_string()),
                query.title,
                query.artist,
            );
            return cached;
        }
        tracing::debug!(
            "SMTC resolve: поиск {:?} / {:?} (токен: {})",
            query.title,
            query.artist,
            if token.is_some() { "да" } else { "нет" },
        );

        // Yandex (auth или anon).
        if !self.backoff_active(CatalogKind::Yandex, now) {
            match self.yandex_search(query, token).await {
                Ok(candidates) => {
                    tracing::debug!(
                        "SMTC resolve: Yandex вернул {} кандидатов",
                        candidates.len()
                    );
                    if let Some(hit) = choose(query, &candidates) {
                        tracing::info!(
                            "SMTC resolve: хит Yandex — {} / {} (cover: {}, url: {})",
                            hit.artist,
                            hit.full_title(),
                            hit.cover_url
                                .as_ref()
                                .map(|u| u.host_str().unwrap_or("?"))
                                .unwrap_or("none"),
                            hit.page_url
                                .as_ref()
                                .map(|u| u.host_str().unwrap_or("?"))
                                .unwrap_or("none"),
                        );
                        let enriched = EnrichedTrack::from(hit.clone());
                        self.cache_put(key, Some(enriched.clone()), now);
                        return Some(enriched);
                    }
                    tracing::debug!("SMTC resolve: Yandex — строгая проверка не прошла");
                }
                Err(unavailable) => {
                    if unavailable {
                        tracing::warn!("SMTC resolve: Yandex недоступен, backoff 60с");
                        self.mark_unavailable(CatalogKind::Yandex, now);
                    }
                }
            }
        } else {
            tracing::debug!("SMTC resolve: Yandex в backoff, пропускаю");
        }

        // iTunes.
        let now = Instant::now();
        if !self.backoff_active(CatalogKind::ITunes, now) {
            if !self.itunes_budget_ok(now) {
                tracing::debug!("iTunes: бюджет 18/мин исчерпан");
            } else {
                match self.itunes_search(query).await {
                    Ok(candidates) => {
                        tracing::debug!(
                            "SMTC resolve: iTunes вернул {} кандидатов",
                            candidates.len()
                        );
                        if let Some(hit) = choose(query, &candidates) {
                            tracing::info!(
                                "SMTC resolve: хит iTunes — {} / {} (cover: {})",
                                hit.artist,
                                hit.full_title(),
                                hit.cover_url
                                    .as_ref()
                                    .map(|u| u.host_str().unwrap_or("?"))
                                    .unwrap_or("none"),
                            );
                            let enriched = EnrichedTrack::from(hit.clone());
                            self.cache_put(key, Some(enriched.clone()), now);
                            return Some(enriched);
                        }
                        tracing::debug!("SMTC resolve: iTunes — строгая проверка не прошла");
                    }
                    Err(unavailable) => {
                        if unavailable {
                            tracing::warn!("SMTC resolve: iTunes недоступен, backoff 60с");
                            self.mark_unavailable(CatalogKind::ITunes, now);
                        }
                    }
                }
            }
        } else {
            tracing::debug!("SMTC resolve: iTunes в backoff, пропускаю");
        }

        // Deezer.
        let now = Instant::now();
        if !self.backoff_active(CatalogKind::Deezer, now) {
            match self.deezer_search(query).await {
                Ok(candidates) => {
                    tracing::debug!(
                        "SMTC resolve: Deezer вернул {} кандидатов",
                        candidates.len()
                    );
                    if let Some(hit) = choose(query, &candidates) {
                        tracing::info!(
                            "SMTC resolve: хит Deezer — {} / {} (cover: {})",
                            hit.artist,
                            hit.full_title(),
                            hit.cover_url
                                .as_ref()
                                .map(|u| u.host_str().unwrap_or("?"))
                                .unwrap_or("none"),
                        );
                        let enriched = EnrichedTrack::from(hit.clone());
                        self.cache_put(key, Some(enriched.clone()), now);
                        return Some(enriched);
                    }
                    tracing::debug!("SMTC resolve: Deezer — строгая проверка не прошла");
                }
                Err(unavailable) => {
                    if unavailable {
                        tracing::warn!("SMTC resolve: Deezer недоступен, backoff 60с");
                        self.mark_unavailable(CatalogKind::Deezer, now);
                    }
                }
            }
        }

        self.cache_put(key, None, Instant::now());
        None
    }

    /// Yandex `/search`: сначала сырой запрос, затем нормализованный
    /// (как `YandexCatalog.search`). `Err(true)` = недоступен (backoff).
    async fn yandex_search(
        &self,
        query: &SmtcQuery,
        token: Option<&str>,
    ) -> Result<Vec<TrackCandidate>, bool> {
        let raw = format!("{} {}", query.artist.trim(), query.title.trim())
            .trim()
            .to_string();
        let norm = format!("{} {}", normalize(&query.artist), normalize(&query.title));
        let mut texts = vec![raw];
        if !norm.trim().is_empty() && normalize(&texts[0]) != norm {
            texts.push(norm);
        }
        let mut out = Vec::new();
        for text in texts.iter().take(2) {
            if text.trim().is_empty() {
                continue;
            }
            match self.yandex_search_once(text, token).await {
                Ok(mut tracks) => {
                    out.append(&mut tracks);
                    if choose(query, &out).is_some() {
                        break;
                    }
                }
                Err(unavailable) => {
                    if unavailable {
                        return Err(true);
                    }
                }
            }
        }
        Ok(out)
    }

    async fn yandex_search_once(
        &self,
        text: &str,
        token: Option<&str>,
    ) -> Result<Vec<TrackCandidate>, bool> {
        let mut req = self
            .http
            .get("https://api.music.yandex.net/search")
            .query(&[
                ("text", text),
                ("type", "track"),
                ("page", "0"),
                ("nocorrect", "true"),
            ])
            .header("X-Yandex-Music-Client", "YandexMusicAndroid/24023621");
        if let Some(token) = token.map(str::trim).filter(|t| !t.is_empty()) {
            req = req.header("Authorization", format!("OAuth {token}"));
        }
        let response = req.send().await.map_err(|err| {
            tracing::debug!("Yandex search недоступен: {err}");
            true
        })?;
        match response.status().as_u16() {
            401 | 403 => {
                tracing::debug!("Yandex search: 401/403 (анонимный режим?)");
                return Err(true);
            }
            200 => {}
            code => {
                tracing::debug!("Yandex search: статус {code}");
                return Ok(Vec::new());
            }
        }
        let body = response.bytes().await.map_err(|_| true)?;
        if body.len() > 2_000_000 {
            return Err(true);
        }
        let payload: serde_json::Value = serde_json::from_slice(&body).map_err(|_| false)?;
        Ok(parse_yandex_search(&payload))
    }

    async fn itunes_search(&self, query: &SmtcQuery) -> Result<Vec<TrackCandidate>, bool> {
        let term = format!("{} {}", query.artist.trim(), query.title.trim());
        let rows = self.itunes_search_rows(&term, None).await?;
        let mut tracks = itunes_rows_to_candidates(&rows);
        if choose(query, &tracks).is_some() {
            return Ok(tracks);
        }
        // Fallback 1: поиск по артисту (до 200), затем lookup по альбомам.
        let artist_rows = self
            .itunes_search_rows(query.artist.trim(), Some("artistTerm"))
            .await?;
        tracks.extend(itunes_rows_to_candidates(&artist_rows));
        if choose(query, &tracks).is_some() {
            return Ok(tracks);
        }
        let mut album_ids: Vec<i64> = Vec::new();
        for row in &artist_rows {
            let same_artist = row
                .get("artistName")
                .and_then(|v| v.as_str())
                .map(|name| normalize(name) == normalize(&query.artist))
                .unwrap_or(false);
            if same_artist
                && let Some(id) = row.get("collectionId").and_then(|v| v.as_i64())
                && !album_ids.contains(&id)
            {
                album_ids.push(id);
            }
        }
        // Альбомы с тем же названием — вперёд (как сортировка у FozerG).
        album_ids.sort_by_key(|id| {
            let title = artist_rows
                .iter()
                .find(|r| r.get("collectionId").and_then(|v| v.as_i64()) == Some(*id))
                .and_then(|r| r.get("collectionName"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            normalize(title) != normalize(&query.album)
        });
        for album_id in album_ids.into_iter().take(4) {
            match self.itunes_lookup(album_id).await {
                Ok(rows) => {
                    tracks.extend(itunes_rows_to_candidates(&rows));
                    if choose(query, &tracks).is_some() {
                        break;
                    }
                }
                Err(true) => return Err(true),
                Err(false) => continue,
            }
        }
        Ok(tracks)
    }

    async fn itunes_search_rows(
        &self,
        term: &str,
        attribute: Option<&str>,
    ) -> Result<Vec<serde_json::Value>, bool> {
        if term.trim().is_empty() {
            return Ok(Vec::new());
        }
        let limit = if attribute.is_some() { "200" } else { "25" };
        let mut req = self.http.get("https://itunes.apple.com/search").query(&[
            ("term", term),
            ("media", "music"),
            ("entity", "song"),
            ("country", "US"),
            ("limit", limit),
        ]);
        if let Some(attr) = attribute {
            req = req.query(&[("attribute", attr)]);
        }
        let response = req
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36")
            .header("Accept", "application/json, text/plain, */*")
            .header("Accept-Language", "ru-RU,ru;q=0.9,en-US;q=0.8,en;q=0.7")
            .send()
            .await
            .map_err(|err| {
                tracing::debug!("iTunes search недоступен: {err}");
                true
            })?;
        if !response.status().is_success() {
            return Err(true);
        }
        let body = response.bytes().await.map_err(|_| true)?;
        if body.len() > 2_000_000 {
            return Err(true);
        }
        let payload: serde_json::Value = serde_json::from_slice(&body).map_err(|_| false)?;
        Ok(payload
            .get("results")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default())
    }

    async fn itunes_lookup(&self, album_id: i64) -> Result<Vec<serde_json::Value>, bool> {
        let response = self
            .http
            .get("https://itunes.apple.com/lookup")
            .query(&[
                ("id", album_id.to_string()),
                ("entity", "song".to_string()),
                ("country", "US".to_string()),
                ("limit", "200".to_string()),
            ])
            .send()
            .await
            .map_err(|_| true)?;
        if !response.status().is_success() {
            return Err(true);
        }
        let body = response.bytes().await.map_err(|_| true)?;
        if body.len() > 2_000_000 {
            return Err(true);
        }
        let payload: serde_json::Value = serde_json::from_slice(&body).map_err(|_| false)?;
        Ok(payload
            .get("results")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default())
    }

    async fn deezer_search(&self, query: &SmtcQuery) -> Result<Vec<TrackCandidate>, bool> {
        let q = format!("{} {}", query.artist.trim(), query.title.trim());
        if q.trim().is_empty() {
            return Ok(Vec::new());
        }
        let response = self
            .http
            .get("https://api.deezer.com/search")
            .query(&[("q", q), ("limit", "25".to_string())])
            .send()
            .await
            .map_err(|err| {
                tracing::debug!("Deezer search недоступен: {err}");
                true
            })?;
        if !response.status().is_success() {
            return Err(true);
        }
        let body = response.bytes().await.map_err(|_| true)?;
        if body.len() > 2_000_000 {
            return Err(true);
        }
        let payload: serde_json::Value = serde_json::from_slice(&body).map_err(|_| false)?;
        let rows = payload
            .get("data")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(rows.iter().filter_map(parse_deezer_row).collect())
    }
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct ITunesRow {
    kind: Option<String>,
}

/// `results[]` iTunes → кандидаты (только `kind == "song"`).
fn itunes_rows_to_candidates(rows: &[serde_json::Value]) -> Vec<TrackCandidate> {
    rows.iter().filter_map(parse_itunes_row).collect()
}

fn parse_itunes_row(row: &serde_json::Value) -> Option<TrackCandidate> {
    if row.get("kind").and_then(|v| v.as_str()) != Some("song") {
        return None;
    }
    let title = row
        .get("trackName")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if title.is_empty() {
        return None;
    }
    let artist = row
        .get("artistName")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let album = row
        .get("collectionName")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let duration_secs = row
        .get("trackTimeMillis")
        .and_then(|v| v.as_u64())
        .map(|ms| ms as f64 / 1000.0)
        .unwrap_or(0.0);
    let cover_url = row
        .get("artworkUrl100")
        .and_then(|v| v.as_str())
        .and_then(build_itunes_cover);
    let page_url = row
        .get("trackViewUrl")
        .and_then(|v| v.as_str())
        .and_then(|s| Url::parse(s).ok())
        .filter(|u| u.scheme() == "https");
    Some(TrackCandidate {
        title,
        version: String::new(),
        artist,
        album,
        duration_secs,
        cover_url,
        page_url,
        source: TrackSource::ITunes,
    })
}

fn parse_deezer_row(row: &serde_json::Value) -> Option<TrackCandidate> {
    let title = row
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if title.is_empty() {
        return None;
    }
    Some(TrackCandidate {
        title,
        version: String::new(),
        artist: row
            .get("artist")
            .and_then(|v| v.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        album: row
            .get("album")
            .and_then(|v| v.get("title"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        duration_secs: row
            .get("duration")
            .and_then(|v| v.as_u64())
            .map(|s| s as f64)
            .unwrap_or(0.0),
        cover_url: row
            .get("album")
            .and_then(|v| v.get("cover_xl"))
            .and_then(|v| v.as_str())
            .and_then(build_deezer_cover),
        page_url: row
            .get("link")
            .and_then(|v| v.as_str())
            .and_then(|s| Url::parse(s).ok())
            .filter(|u| u.scheme() == "https"),
        source: TrackSource::Deezer,
    })
}

/// `result.tracks.results[]` Яндекса → кандидаты.
fn parse_yandex_search(payload: &serde_json::Value) -> Vec<TrackCandidate> {
    payload
        .get("result")
        .and_then(|r| r.get("tracks"))
        .and_then(|t| t.get("results"))
        .and_then(|v| v.as_array())
        .map(|results| results.iter().filter_map(parse_yandex_row).collect())
        .unwrap_or_default()
}

fn parse_yandex_row(item: &serde_json::Value) -> Option<TrackCandidate> {
    let id_raw = item.get("id").and_then(|v| {
        v.as_str()
            .map(str::to_string)
            .or_else(|| v.as_i64().map(|n| n.to_string()))
    })?;
    let track_id = id_raw.split(':').next().unwrap_or(&id_raw).to_string();
    if track_id.trim().is_empty() {
        return None;
    }
    let title = item
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if title.is_empty() {
        return None;
    }
    let artists: Vec<String> = item
        .get("artists")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| a.get("name").and_then(|v| v.as_str()))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let album_obj = item
        .get("albums")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .cloned();
    let album_id = album_obj
        .as_ref()
        .and_then(|a| {
            a.get("id").and_then(|v| {
                v.as_str()
                    .map(str::to_string)
                    .or_else(|| v.as_i64().map(|n| n.to_string()))
            })
        })
        .unwrap_or_default();
    let album_title = album_obj
        .as_ref()
        .and_then(|a| a.get("title"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let cover_uri = album_obj
        .as_ref()
        .and_then(|a| a.get("coverUri"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            item.get("og-image")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });
    let cover_url = cover_uri
        .as_deref()
        .and_then(crate::yandex::metadata::build_https_cover);
    let page_url = if album_id.trim().is_empty() {
        Url::parse(&format!("https://music.yandex.ru/track/{track_id}")).ok()
    } else {
        Url::parse(&format!(
            "https://music.yandex.ru/album/{album_id}/track/{track_id}"
        ))
        .ok()
    };
    Some(TrackCandidate {
        title,
        version: item
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        artist: artists.join(", "),
        album: album_title,
        duration_secs: item
            .get("durationMs")
            .and_then(|v| v.as_u64())
            .map(|ms| ms as f64 / 1000.0)
            .unwrap_or(0.0),
        cover_url,
        page_url,
        source: TrackSource::Yandex,
    })
}

/// iTunes `artworkUrl100` → 600px + проверка хоста.
fn build_itunes_cover(raw: &str) -> Option<Url> {
    let upgraded = raw.replace("100x100", "600x600");
    let url = Url::parse(&upgraded).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    let host = url.host_str().unwrap_or("");
    if host.ends_with("mzstatic.com") {
        Some(url)
    } else {
        None
    }
}

/// Deezer `cover_xl` → проверка хоста.
fn build_deezer_cover(raw: &str) -> Option<Url> {
    let url = Url::parse(raw.trim()).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    let host = url.host_str().unwrap_or("");
    if host.ends_with("dzcdn.net") {
        Some(url)
    } else {
        None
    }
}

/// Статус SMTC для пропуска: paused/stopped тоже ищем (обложка нужна и на паузе).
#[allow(dead_code)]
pub fn status_allows_lookup(status: PlaybackStatus) -> bool {
    !matches!(status, PlaybackStatus::Stopped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn itunes_cover_upgrades_and_validates_host() {
        let url =
            build_itunes_cover("https://is1-ssl.mzstatic.com/image/thumb/Music/100x100bb.jpg")
                .expect("valid");
        assert!(url.as_str().contains("600x600"));
        assert!(build_itunes_cover("https://evil.example/100x100.jpg").is_none());
        assert!(build_itunes_cover("http://is1-ssl.mzstatic.com/a/100x100bb.jpg").is_none());
    }

    #[test]
    fn deezer_cover_validates_host() {
        assert!(build_deezer_cover("https://e-cdns-images.dzcdn.net/images/cover/1.jpg").is_some());
        assert!(build_deezer_cover("https://evil.example/cover.jpg").is_none());
    }

    #[test]
    fn itunes_row_filters_non_songs() {
        let row = serde_json::json!({
            "kind": "music-video",
            "trackName": "Track",
            "artistName": "Artist",
        });
        assert!(parse_itunes_row(&row).is_none());
        let row = serde_json::json!({
            "kind": "song",
            "trackName": "Track",
            "artistName": "Artist",
            "collectionName": "Album",
            "trackTimeMillis": 200000,
            "artworkUrl100": "https://is1-ssl.mzstatic.com/image/100x100bb.jpg",
            "trackViewUrl": "https://music.apple.com/us/song/1",
        });
        let track = parse_itunes_row(&row).expect("song");
        assert_eq!(track.duration_secs, 200.0);
        assert_eq!(track.source, TrackSource::ITunes);
    }

    #[test]
    fn yandex_row_builds_album_track_url() {
        let item = serde_json::json!({
            "id": "123:456",
            "title": "Track",
            "version": "",
            "durationMs": 200000,
            "artists": [{"name": "Artist"}],
            "albums": [{"id": 789, "title": "Album", "coverUri": "avatars.yandex.net/get-x/y/%%"}],
        });
        let track = parse_yandex_row(&item).expect("row");
        assert_eq!(
            track.page_url.as_ref().map(Url::as_str),
            Some("https://music.yandex.ru/album/789/track/123")
        );
        assert!(track.cover_url.is_some());
    }

    #[test]
    fn cache_key_normalizes() {
        let a = SmtcQuery {
            title: "  Track! ".into(),
            artist: "Artist".into(),
            album: "".into(),
            duration_secs: 200.4,
        };
        let b = SmtcQuery {
            title: "track".into(),
            artist: "artist".into(),
            album: "".into(),
            duration_secs: 200.0,
        };
        assert_eq!(TrackResolver::cache_key(&a), TrackResolver::cache_key(&b));
    }
}
