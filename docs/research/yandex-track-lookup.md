# Поиск треков Яндекс Музыки для SMTC-режима: обложки, ссылки, метаданные для Discord

Дата исследования: 2026-10-19
Статус: частично реализовано (2026-10-19): `sources/smtc/matching.rs`,
`sources/smtc/catalogs.rs`, `sources/smtc/enrichment.rs` + честная кнопка
(`track_source` → label). См. §9.
Связано: `sources/smtc/source.rs`, `sources/model.rs`, `yandex/metadata.rs`, `yandex/protocol.rs`, `presence/mapper.rs`, `docs/research/ynison-protocol.md`, `docs/research/discord-ipc.md`

## 1. Постановка задачи

SMTC-источник (`sources/smtc/source.rs::poll_smtc`) сейчас даёт для Discord только то, что отдаёт Windows:

- `title`, `artists[]` (split по `,`), `album`, `duration_ms`, `position_ms`, `status`
- `track_id = None`, `cover_url = None`, `track_url = None`

Поэтому `presence/mapper.rs` в SMTC-режиме всегда:

- ставит `large_image = "yandex-music-default"` (статический asset key из Developer Portal),
- не ставит кнопку (`button_url = None`, т.к. нет HTTPS-ссылки).

Ynison-режим этой проблемы не имеет: там есть `playableId` + REST-доделка
`GET /tracks?trackIds=` (`yandex/metadata.rs::MetadataClient`) → `cover_url` + `track_url`.

Задача: научить SMTC-режим искать трек в каталоге Яндекс Музыки по строке
`title + artists (+album, duration)` и доставать `cover_url`, `track_url`
(и при желании уточнять `artists/album/duration`), чтобы Discord-карточка
была такой же богатой, как в Ynison-режиме.

Ограничение из `yandex/mod.rs`: модуль «не выполняет поиск треков по строкам».
Этот документ предлагает, как снять ограничение аккуратно.

## 2. Что нужно Discord (кратко, подробности — в `discord-ipc.md`)

- `assets.large_image`: можно класть **полный `https://` URL** при каждом
  `SET_ACTIVITY`. Клиент Discord сам проксирует (`mp:`), регистрировать
  ничего не надо. Заготовленный asset key нужен только как fallback.
- Тип активности — `Listening = 2` (уже так, `discord/models.rs::ACTIVITY_LISTENING`).
- Лимит: **5 обновлений / 20 с**. Поиск по смене трека в него укладывается,
  поиск на каждый SMTC-тик (1 с) — нет.
- `details/state/large_text`: де-факто держать ≤ 128 символов (`MAX_FIELD_LEN`),
  уже делается в `mapper`.
- Кнопка: `label 1–32`, `url 1–512`, максимум 2. Сейчас одна:
  «Открыть в Яндекс Музыке».
- CSP фронтенда уже разрешает только
  `avatars.yandex.net / avatars.mds.yandex.net` по HTTPS — новый поиск
  не должен вводить другие CDN без обновления CSP и allowlist
  (`protocol.rs::ALLOWED_COVER_HOSTS`, `metadata.rs::build_https_cover`).

Что нужно для «богатой» карточки из поиска:

| Поле `PlaybackState` | Откуда взять | Как строить |
|---|---|---|
| `cover_url` | `coverUri` из `/search` или `/tracks` | `https://` + `replace("%%","400x400")`, только allowlist-хосты |
| `track_url` | `id` + `albums[0].id` из `/search` | `https://music.yandex.ru/album/{albumId}/track/{trackId}`, fallback `https://music.yandex.ru/track/{id}` |
| `title/artists/album/duration` | тот же ответ | только для отображения/скоринга, `track_id` у SMTC по-прежнему `None` (непрозрачный ID каталога можно класть опционально, но не обязан) |

Формат `coverUri` (подтверждено `ynison-protocol.md` §4.2 и `metadata.rs`):
`avatars.yandex.net/get-music-content/<sid>/<hash>/%%` (без схемы) или полный
`https://.../%%`. `%%` → размер (`50x50`, `200x200`, `400x400`, …).
Сейчас везде используется `400x400` — оставить.

Формат `trackId`: иногда с региональным суффиксом `"123:456"` — перед
подстановкой в URL делать `split(":")[0]` (как в FozerG `yandex.py`).

## 3. Вариант A (рекомендуемый, когда токен есть): авторизованный `/search`

Работает в режимах `auto` (SMTC-fallback при живом токене) и
`media_control` + сохранённый токен. Переиспользует существующую
инфраструктуру `MetadataClient` (токен из Credential Manager, `reqwest`,
LRU 200, отрицательный кеш).

### 3.1. Endpoint

```http
GET https://api.music.yandex.net/search?text={query}&type=track&page=0&nocorrect=false
Authorization: OAuth {token}
X-Yandex-Music-Client: YandexMusicAndroid/24023621
```

- `type=track` (в разных SDK также `tracks`/`all`; для нас фиксировать
  `track`, проверено в `lavende-core`, `Rustalink`, `yamuse`).
- `nocorrect=false` — оставить исправление опечаток сервера, но скоринг
  делать по полю `misspell_*` ответа (см. §3.3).
- Ответ: `{"result": {"tracks": {"total","perPage","results": [...]},
  "best": {...}, "misspell_*": ...}}`.
- Каждый `results[]`: `id`, `title`, `durationMs`, `artists[]:{name}`,
  `albums[]:{id,title,coverUri}`.
- Ошибки: `401/403` → `InvalidToken` (как в `metadata.rs::fetch`,
  дальше `RequiresAuth` + SMTC продолжает без обогащения);
  `!success` → `YandexUnavailable`, live-state остаётся главным.

Это тот же приватный API, что уже используется для `/tracks`
(ToS-риск не возрастает: те же хост, токен, заголовки).

### 3.2. Построение запроса

SMTC даёт `title`, `artists[]`, `album`. Запрос строить так:

1. `query = "{artists[0]} — {title}"`, если есть артист, иначе `"{title}"`.
   Альбом в запрос не добавлять (ухудшает recall у синглов/радио).
2. Нормализация перед отправкой и перед скорингом: `trim`, схлопывание
   пробелов, `to_lowercase`, выкидывание `(пауза)`-хвоста маппера,
   унификация `ё→е`, `—/– → -`. Хранить `normalized_query` как ключ кеша.
3. `page=0`, `type=track` всегда. Пагинацию не делать (топ-1 достаточно).

### 3.3. Скоринг и выбор кандидата

Брать `results[0..5]`, считать вес, принимать при `score >= порога`,
иначе — «не найдено» (отрицательный кеш, карточка без обложки/кнопки,
как сейчас).

Предлагаемая формула (простая, детерминированная, тестируемая):

```text
title_exact  = normalized(title_smtc) == normalized(title_api) ? 50 : 0
title_fuzzy  = иначе max(0, 30 - levenshtein*5)   // опечатки, «(Remaster)»
artist_match = артист SMTC есть среди artists_api (после нормализации) ? 30 : 0
album_bonus  = album_smtc непуст и равен albums[0].title ? 10 : 0
duration_ok  = |dur_smtc - durationMs| <= 3000 ? 10 : (<= 8000 ? 5 : -20)
score = title_exact/title_fuzzy + artist_match + album_bonus + duration_ok
порог accept = 70
```

Дополнительно:

- `playableType != TRACK` / `type != music` — пропускать (как FozerG).
- Если `best.type == "track"` и `best.result.id` есть — проверять его первым.
- `duration_ms` у SMTC может не быть (браузеры) — тогда вес длительности `0`,
  порог не менять.
- Радио/нейро (`durationMs == 0`): не обогащать обложкой трека, оставить fallback.

### 3.4. Доделка по `/tracks` (опционально, но желательно)

`/search` иногда отдаёт урезанный `coverUri`. Надёжный путь, как в Ynison:

1. `search` → взять `id` победителя,
2. `GET /tracks?trackIds={id}` (параметр именно `trackIds`, не `ids` —
   проверено на продакшене, см. комментарий в `metadata.rs:187`),
3. из ответа взять канонический `coverUri + albums[0].id + durationMs`.

Два запроса только на смену трека — в лимит Discord укладывается.
Кешировать итог по `normalized_query → TrackMetadata + track_url`.

## 4. Вариант B (когда токена нет): что реально доступно

Чистый `media_control` без токена — самый частый кейс жалоб
(«работает захват из Windows, а обложки нет»). Эталонная реализация каскада
без токена уже есть и разобрана построчно: **FozerG/WinYandexMusicRPC**
(`ymrpc/catalogs.py`, `ymrpc/search.py`, `ymrpc/matching.py`,
`ymrpc/presence.py`, `ymrpc/worker.py`). Логика там такая:

```text
SMTC-снапшот → TrackResolver [YandexCatalog → ITunesCatalog → DeezerCatalog]
  → строгая проверка choose/compare → matched → cover + url + app_url
  → иначе presence скрыт («трек не найден»), а не показан с чужой обложкой
```

То есть «spike» ниже — это не прод-код, а короткая ручная проверка
(`curl`, 0.5–1 день): убедиться, что хендлер/endpoint отвечает из сети
пользователя без токена, зафиксировать реальный JSON и только потом писать
модуль. Сам прод-путь для «без токена» — это каскад B1 → B3a → B3b,
скопированный с WinYandexMusicRPC.

### B1. Яндекс-поиск без токена (первый в каскаде)

WinYandexMusicRPC делает именно так: `YandexCatalog` создаётся с `Client()`
без токена (`ymrpc/yandex.py::YandexCatalog.search`): запрос
`client.search(text, type_="track", nocorrect=True, timeout=4)` сначала сырым
`"{artist} {title}"`, затем нормализованным (той же строкой после
`normalize()`), берётся до 25 кандидатов, фильтр
`type in (None, "music", "track")`. Если анонимный `/search` отвечает —
маппинг как в варианте A (`og_image || cover_uri` → `400x400`,
`album/{albumId}/track/{trackId}` + `yandexmusic://` для второй кнопки;
в нашем MVP вторая кнопка отсутствует — игнорировать).

Если анонимный `/search` отдаёт `401` — каталог помечается `unavailable`
(backoff 60 с, `search.py::TrackResolver._search`) и каскад идёт дальше,
а не падает. Это и есть ответ на «spike»: сначала дёрнуть `/search` без
токена из целевой сети и посмотреть, `200` или `401`.

Запасной хендлер B1, если анонимный `/search` закрыт. Используется SearXNG
(`searx/engines/yandex_music.py`, без API-ключа):

```http
GET https://music.yandex.ru/handlers/music-search.jsx?text={query}&type=track&page=0
```

Возвращает JSON `{"tracks": {"items": [{"id","title","artists","albums"}]}}`.
Ссылка собирается как `https://music.yandex.ru/album/{albumId}/track/{trackId}`.

- Плюс: токен не нужен вообще, хост тот же, что у веб-плеера.
- Минус: недокументирован, может потребовать cookies/`Session_id`/капчу,
  меняется без notice. Держать только как best-effort с коротким таймаутом
  (5–8 с) и graceful-degradation. Перед внедрением — ручной spike
  `curl` из Windows-сети пользователя.

### B2. Сторонний резолвер/прокси (не рекомендовать по умолчанию)

`SKProCH/YandexMusicResolver` умеет «без авторизации», но это обёртка
над тем же приватным API со своими кредами/прокси. Для десктоп-приложения
это лишний посредник + чужой ToS + латентность. WinYandexMusicRPC обходится
без него (прямые `Client()` + iTunes + Deezer) — и нам он не нужен.
Только если B1 умрёт.

### B3. Каскад iTunes → Deezer как в WinYandexMusicRPC (без токена)

Это не «опциональный второй шанс», а штатные ступени 2 и 3 каскада.
Оба источника — публичные, без auth, стабильные и легальные.
Порядок важен: сначала iTunes (точнее метаданные + строгий скоринг),
затем Deezer (шире покрытие, слабее проверка).

#### B3a. iTunes Search API (`ymrpc/catalogs.py::ITunesCatalog`)

```http
GET https://itunes.apple.com/search?term={artist title}&media=music&entity=song&country=US&limit=25
# fallback 1: term={artist}&attribute=artistTerm&limit=200 (открыть альбомы артиста)
# fallback 2: GET https://itunes.apple.com/lookup?id={collectionId}&entity=song&country=US&limit=200
```

- Маппинг: `trackName → title`, `artistName → artist`,
  `collectionName → album`, `trackTimeMillis/1000 → duration`,
  `artworkUrl100 → cover`, `trackViewUrl → url`. Фильтр `kind == "song"`.
- Обложку апскейлить заменой `100x100 → 600x600` в URL (Discord сам
  проксирует; 100px в оригинале у FozerG — мыльно, лучше 600).
- Лимит: ≤ 18 запр/мин на процесс (у FozerG `deque` в `CatalogHttp.get`),
  таймаут 4 с, cap ответа 2 МБ, `User-Agent: Chrome/140`, `Accept-Encoding: gzip`.
- `country=US` фиксирован (стабильность выдачи; у нас UI русский, но каталог
  американский полнее; локаль поиска не менять без spike).

#### B3b. Deezer Search API (`ymrpc/catalogs.py::DeezerCatalog`)

```http
GET https://api.deezer.com/search?q={artist title}&limit=25
```

- Маппинг: `title → title`, `artist.name → artist`, `album.title → album`,
  `duration (с) → duration`, `album.cover_xl (1000px) → cover`, `link → url`.
- Без auth, таймаут тот же 4 с. Отдельного artist-fallback нет — одна ступень.

#### B3c. Строгая проверка — портировать `matching.py` один в один

Ключевое отличие WinYandexMusicRPC от «взять первый результат»: кандидат
принимается только после `choose(media, tracks)`. Иначе — `not_found`,
и presence **скрывается** (`show_unknown_media=false`), а не показывается
с чужой обложкой. Портировать в Rust (`sources/smtc/matching.rs`):

- `normalize`: NFKC + casefold + `ё→е`, всё не-алфанумерик → пробел,
  схлопнуть пробелы. Совпадать должны нормализованные строки.
- `title_parts`: вырезать `feat./ft./featuring` из `[...]`/`(...)` и хвоста
  в множество `guests` (фитующие входят в множество артистов).
- `base_title`: снять трейлинг-суффиксы `[...]`/`(...)` до упора
  (`(Remaster)`, `(Live)`), отрезать версию после ` — `.
- `compare(media, track) → (bool, reason)`:
  1. пустой title — сразу `false`;
  2. title равен полному ИЛИ базовые title равны (иначе `false`);
     при совпадении только баз (`different_suffix`) длительность обязательна
     для проверки;
  3. `wanted = artists(media) ∪ guests ⊆ actual = artists(track) ∪ guests`
     — мультиартисты покрываются подмножеством, а не равенством;
  4. длительность: `|a−b| ≤ max(8с, 4%)`, иначе `false`.
- `choose`: среди прошедших — сначала совпавший альбом
  (`normalize(album)`), затем ближайшая длительность, затем наличие обложки.

Наш скоринг из §3.3 для авторизованного Яндекс-поиска можно оставить как есть,
но для iTunes/Deezer-ступени использовать именно этот строгий `compare/choose`
(двух разных скорингов в кодовой базе быть не должно — унифицировать на нём).

#### B3d. Политика кнопок/обложек для чужих каталогов

В `presence.py::build_payload`: `large_image = track.cover` (любой https),
кнопки — только если есть `url`/`app_url`. Для iTunes/Deezer-трека `url` —
это `trackViewUrl`/`deezer.com/...`, а не Яндекс. Подпись
«Открыть в Яндекс Музыке» (`discord/models.rs::OPEN_IN_YANDEX_LABEL`) с такой
ссылкой — враньё. Варианты:

1. (рекомендуется, как у FozerG) разрешить до 2 кнопок и подписывать честно:
   Яндекс-хит → «Открыть в Яндекс Музыке»; iTunes-хит → кнопка ведёт на
   `trackViewUrl` (label другой, напр. «Открыть в iTunes»); Deezer-хит —
   аналогично. Требует расширения `DiscordActivity` (сейчас одна кнопка).
2. (консервативно, без расширения модели) чужой хит даёт только обложку,
   кнопка скрывается. Яндекс-ссылка не фабрикуется из чужого ID.
3. Запрещено: строить `music.yandex.ru/search?text=` как «кнопку трека» —
   это выдача, а не трек, вводит в заблуждение.

Дополнительно: hosts `is1-ssl.mzstatic.net` (iTunes) и `e-cdns-images.dzcdn.net`
(Deezer) придётся добавить в CSP фронтенда и в allowlist обложек
(сейчас только `avatars.*`). Без этого `mapper` их отрежет.

### B5. SMTC-thumbnail (отклонить)

`TryGetMediaPropertiesAsync` + `GetThumbnailAsync` отдаёт локальный
`RandomAccessStream` (байты), а Discord IPC принимает только URL/asset-key.
Чтобы показать его, байты надо куда-то загрузить (свой CDN = хостинг,
трафик, приватность прослушки пользователя) или предварительно залить
сотни обложек в Developer Portal (не масштабируется). WinYandexMusicRPC
этот путь тоже не использует — только каталоги выше. Не делать.

## 5. Рекомендуемая стратегия (итог, как у WinYandexMusicRPC)

```text
SMTC Identity-событие (новый title/artists/album)
  │
  ├─ токен есть? ── да ──→ A: /search (type=track, auth) → choose/compare → /tracks
  │                              │ hit → cover_url + track_url (album/track)
  │                              │ miss/401 → дальше по каскаду B3a→B3b
  │
  └─ токена нет? ──→ B1: /search анонимно ─→ hit → cover_url + track_url
                         │ miss/401 → B3a iTunes → choose → hit? → cover (+кнопка iTunes/нет)
                         │            │ miss → B3b Deezer → choose → hit? → cover (+кнопка Deezer/нет)
                         │            │        │ miss → fallback-арт, без кнопки
                         │            │        │ (а не первая попавшаяся обложка)
                         └─ (запасной B1) handlers/music-search.jsx, если /search закрыт
```

 Правила, общие для A и B (все — из `ymrpc/search.py + worker.py`):

- Искать **только на `Change::Identity`** (смена title/artists/album/duration),
  никогда на `Same`/`Seek` и никогда на каждый тик. SMTC-детектор уже
  классифицирует это (`source.rs::classify_change`). У FozerG тот же триггер:
  `media.identity = (source, title, artist)`.
- Один поиск — один фоновый worker (`ThreadPoolExecutor(max_workers=1)` у них;
  у нас — отдельная tokio-задача enrichment). Опрос SMTC 1 с не блокировать;
  `Identity → lookup → merge → SourceEvent::State` с тем же `observed_at`.
- Отмена устаревшего поиска: если трек переключился раньше ответа —
  результат выкидывается (`cancel Event` + `generation` у них; у нас —
  `CancellationToken`/сравнение identity перед merge). FozerG дополнительно
  перепроверяет `source.read()` перед публикацией (трек мог проскочить).
- Пока идёт lookup — вести себя как `searching`: либо держать прошлую
  карточку (наш grace), либо скрывать (их `show_unknown_media=false`).
  Публиковать непроверенный трек с чужой обложкой — запрещено.
- Без артиста — поиск не делать вообще (`if not media.artist.strip(): skip`,
  иначе iTunes/Deezer дадут мусор).
- Кеш: LRU 128–200 + TTL (у них `OrderedDict`, hit 3600 с, miss 60 с;
  у нас `metadata.rs::LruCache` 200 — добавить TTL по тому же принципу).
  Backoff провайдера 60 с после исключения (`_retry_at`), iTunes отдельно
  throttled 18 запр/мин.
- Логи: запрос — `debug` (без токена/заголовков), `warn` только на недоступность.
  У FozerG логируется каждый кандидат `artist — full_title — duration — reason`.
- При `show_album_artwork=false` / `show_button=false` — поиск не делать вообще
  (настройки уже есть в `AppSettings`). Плюс решить: нужен ли аналог их
  `show_unknown_media` (показывать непроверенное без обложки) или всегда скрывать
  miss (сейчас у нас miss = fallback-арт, что мягче их подхода).

## 6. Предлагаемый план внедрения

1. **Spike (без кода в прод, 0.5–1 день):**
   - `curl` из Windows: `A` с валидным токеном, анонимный `/search` без токена,
     `B3a` iTunes, `B3b` Deezer. Зафиксировать реальные JSON, статусы без токена,
     наличие `coverUri`/`cover_xl`/`artworkUrl100`.
   - Проверить размеры `%%` (`400x400` vs `800x800`) и `600x600` у iTunes
     в Discord-клиенте.
2. **`sources/smtc/matching.rs` (новый, порт `ymrpc/matching.py`):**
   - `normalize`, `title_parts`, `base_title`, `compare`, `choose` один в один,
     включая допуск `max(8с, 4%)` и правило подмножества артистов.
   - Unit-тесты на их кейсах: feat-суффиксы, `(Remaster)`, мультиартисты,
     несовпадение длительности, пустой артист.
3. **`yandex/search.rs` + `sources/smtc/catalogs.rs` (новые):**
   - `SearchClient { token: Option<String>, http }` + каскад
     `Yandex → iTunes → Deezer` с backoff 60 с и throttle iTunes 18/мин.
   - `normalize_query`, `build_track_url(albumId, trackId)`,
     `build_https_cover` — переиспользовать существующий; для iTunes/Deezer
     отдельные билдеры URL + расширение allowlist/CSP.
   - Unit-тесты на фикстурах (ровно как `protocol.rs::tests`): точное совпадение,
     ремикс-мимо, несовпадение длительности, региональный суффикс `id`.
4. **Enrichment-worker для SMTC:**
   - хук после `classify_change == Identity` в `spawn_smtc_source`
     (или отдельный `sources/smtc/enrichment.rs`, чтобы `source.rs` не рос).
   - `token = auth::load_token().ok().flatten()` на момент Identity
     (не кешировать между треками — токен могут удалить).
   - merge: `PlaybackState { cover_url, track_url }` + `track_id = Some(api_id)?`
     (решить явно: хранить ли Яндекс-ID у SMTC-трека; mapper его не требует).
4. **Настройки/UI (минор):**
   - переиспользовать `show_album_artwork/show_button`; решить про аналог их
     `show_unknown_media` / `pause_timeout` и про расширение `DiscordActivity`
     до 2 кнопок (B3d вариант 1) либо «чужая обложка без кнопки» (вариант 2).
   - `schemaVersion 4 → 5` с миграцией (см. `config/model.rs::sanitized`).
5. **Документация:** ADR в `docs/architecture-decisions.md`
   (почему поиск — в `yandex/`, а оркестрация — в `sources/smtc/`,
   почему thumbnail отклонён).

Критерии приёмки:

- SMTC + токен: трек Яндекс Музыки показывает обложку `avatars.*` и кнопку
  `…/album/…/track/…`; Spotify/браузер — либо своя обложка по скорингу,
  либо честный fallback (без ложной Яндекс-кнопки).
- SMTC без токена: не хуже, чем сейчас (нет регрессий, нет спама в сеть,
  нет блокировки опроса).
- `cargo test` зелёный; сеть в unit-тестах замокана.

## 7. Открытые вопросы (закрыть на spike)

- [ ] Анонимный `/search` без токена: `200` или `401` из целевой сети?
  (У FozerG код написан в расчёте на `200`, но с graceful `unavailable`.)
- [ ] `handlers/music-search.jsx` без cookies: отдаёт JSON или редирект на капчу?
  (нужен только если анонимный `/search` закрыт).
- [ ] Канонический `track_url`: `/album/{a}/track/{t}` vs `/track/{t}` — что стабильнее
      открывает десктопный клиент (сейчас `protocol.rs` строит короткую форму)?
- [ ] Кнопки для чужих хитов: расширять `DiscordActivity` до 2 кнопок с честными
      label (B3d-1) или чужая обложка без кнопки (B3d-2)?
- [ ] Miss-политика: fallback-арт с live-текстом (как сейчас) или скрывать presence
      как у FozerG (`show_unknown_media=false`)?
- [ ] Максимальный размер `%%`, который Discord проксирует без даунскейла-артефактов?

## 8. Источники

- `docs/research/ynison-protocol.md` §4.2–§6: `coverUri → https://…/%%`,
  `track URL`, заголовки `Authorization: OAuth` + `X-Yandex-Music-Client`.
- `src-tauri/src/yandex/metadata.rs`: `trackIds` (не `ids`), LRU 200,
  `build_https_cover` allowlist.
- `src-tauri/src/yandex/protocol.rs:344`: `build_track_url`, `ALLOWED_COVER_HOSTS`.
- `docs/research/discord-ipc.md` §2: внешний URL в `large_image` — официально,
  `mp:` руками не строить; §лимиты: 5/20 с, `Listening=2`.
- API-клиенты (референсы поиска, все требуют тот же OAuth, кроме SearXNG):
  `MarshalX/yandex-music-api` (`search`), `thedvxch/yandex-music`,
  `yamuse` (Rust, `search`), `Rustalink` (`/search?type=all`),
  `goym` (`SearchQueryParams`), `acherkashin/yandex-music-open-api`
  (`GET /search`, `GET /search/suggest`, auth `optional` — проверить),
  `winster332/Yandex.Music.Api` (`SearchTrackAsync`).
- SearXNG `searx/engines/yandex_music.py`: `music.yandex.ru/handlers/music-search.jsx`
  без ключа (запасной B1).
- WinYandexMusicRPC (разобран построчно 2026-10-19, MIT): `ymrpc/yandex.py`
  (`YandexCatalog`: анонимный `Client().search`, `og_image || cover_uri`,
  `album/{id}/track/{id}` + `yandexmusic://`), `ymrpc/catalogs.py`
  (`ITunesCatalog`: `/search` + `attribute=artistTerm` + `/lookup`,
  throttle 18/мин, cap 2 МБ, timeout 4 с; `DeezerCatalog`: `api.deezer.com/search`),
  `ymrpc/matching.py` (`normalize/title_parts/base_title/compare/choose`,
  допуск `max(8с, 4%)`, подмножество артистов), `ymrpc/search.py`
  (`TrackResolver`: порядок Yandex→iTunes→Deezer, LRU 128, hit 3600 с / miss 60 с,
  backoff 60 с, отмена по смене identity), `ymrpc/presence.py`
  (`large_image = track.cover`, кнопки только при `url/app_url`),
  `ymrpc/worker.py` (триггер по `identity`, перепроверка перед публикацией,
  `show_unknown_media=false` скрывает непроверенное).
- iTunes Search API (`itunes.apple.com/search`, без auth) — ступень B3a.
- Deezer Search API (`api.deezer.com/search`, без auth) — ступень B3b.
- SMTC-thumbnail: `mpv osdep/win32/smtc.cpp`, `LibSMTC` — локальный
  `RandomAccessStreamReference`, для Discord IPC непригоден (см. §4 Б5).

## 9. Реализация (2026-10-19, решения по вопросам пользователя)

- Кнопка: **одна, с динамическим честным label** (`discord/models.rs::
  button_label_for_source`): Яндекс → «Открыть в Яндекс Музыке»,
  iTunes → «Открыть в iTunes», Deezer → «Открыть в Deezer».
  Источник хранится в `PlaybackState.track_source` (сериализуется как
  `trackSource` во фронтенд, `src/types/app.ts`), legacy без источника —
  дефолт Яндекс. Miss-политика: fallback-арт как раньше, без кнопки.
- Каскад: **сразу полный** `Yandex (auth/anon) → iTunes → Deezer`.
- Модули: `sources/smtc/matching.rs` (порт `compare/choose`, 9 тестов),
  `sources/smtc/catalogs.rs` (`TrackResolver`: кеш LRU 128 hit 3600с/miss 60с,
  backoff 60с, throttle iTunes 18/мин), `sources/smtc/enrichment.rs`
  (догонка только на `Change::Identity`, отмена по поколению, merge сохраняет
  player-правду и `observed_at`).
- Проверено: `cargo test --lib` — 117 passed; `pnpm build` — ок.

## 10. Исправления по обратной связи (2026-10-19)

1. **Удаление токена не обновляло UI.** `runtime::on_token_removed` обновлял
   только арбитра, но не снапшот: `auth_required` оставался `false`, а
   `publish_snapshot` не вызывался (Ynison уже остановлен — нового статуса
   не будет). Исправлено: `set_token_present(false) + publish_snapshot`
   по аналогии с `on_token_saved`. Уведомление и так отправлялось.
2. **Перемотка сбрасывала картинку/кнопку.** Причины: обогащение триггерилось
   только на `Identity`, а подтверждённый `Seek` публиковал сырое состояние
   без обложки; плюс результат fetch-таска нёс протухшую позицию.
   Исправлено архитектурой «кеш в цикле»:
   - `enrichment.rs`: `IdentityKey` (title/artists/album/duration — перемотка
     ключ не меняет), `EnrichmentCache` (cap 32, FIFO), `attach_cached`
     (подкладывает cover/track/source + недостающие album/duration, не трогая
     позицию и `observed_at`), `spawn_fetch` возвращает результат в цикл через
     канал, а не в `SourceEvent`;
   - `source.rs`: дренаж канала каждый тик → `attach_cached` до `classify`;
     новый `Change::Enriched` (тот же трек + подъехали cover/track) публикует
     сразу без похода в сеть; `EmitMemo` запоминает cover/track/source;
     `Seek`-подтверждение публикует уже с кешем; протухшие fetch-результаты
     отбрасываются по поколению и не перезаписывают позицию.
3. **«В приложении есть, в Discord нет».** Добавлены логи на каждом рубеже:
   резолвер (`SMTC resolve: хит/кеш/недоступен/backoff` с хостами),
   цикл (`кеш-хит/промах`, `результат в кеше`), координатор (изменение desired,
   rate-limit задержка, публикация с `image_host` + `button`), Discord IPC
   (`set_activity image/button`). Диагностика: сравнить хост картинки в логах
   резолвера и координатора; если координатор публикует, а Discord не
   показывает — клиент не смог скачать/проксировать URL (смотреть хост).

4. **Раскладка Discord и кнопка в приложении (2026-10-19).** По обратной связи:
   вторая строка — только автор (без `· альбом`), третья строка (`large_text`) —
   альбом (раньше дублировала название). `tidy_separators` удалён за ненадобностью.
   Кнопки «не было нигде»: в карточке приложения ссылка вообще не рендерилась —
   добавлена (`NowPlayingCard.vue`: `trackUrl` + честный `TRACK_SOURCE_LABELS`;
   подпись тумблера в настройках обобщена). Проверка Discord-стороны: конструкция
   пейлоада верна (`Button::new(label, url)`, `buttons(vec![…])` — сверено с
   исходниками крейта 1.1.0), кнопка отсутствует ровно когда нет `track_url`
   (стрикт-матчинг ничего не подтвердил или `showButton` выключен); точки
   диагностики — логи `SMTC resolve` (§10.3).

5. **Выключатель «Публикация в Discord» не убирал presence.** Баг в
   `presence/coordinator.rs::schedule`: ветка `presence_enabled == false`
   обнуляла `published` ДО `execute`, превращая `Pending::Clear` в no-op
   (`published.is_none() → return` — clear-кадр в Discord не уходил), а затем
   безусловно затирала `pending`, убивая retry. Трек вечно висел в Discord.
   Исправлено по образцу штатного Clear-пути: `published` сбрасывает только
   `execute` после успешного `clear()`, retry сохраняется. Проверка вручную:
   выключить тумблер → в логе `Activity очищена`, presence в профиле пропал.
