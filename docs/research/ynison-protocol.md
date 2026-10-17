# Ynison — Yandex Music real-time playback protocol (WebSocket)

Implementable reference for a Rust client. Everything in this document is taken
**verbatim from reading the reference source code** on 2026-10-17. Source files
are cited inline as links to `raw.githubusercontent.com`.

## Reference sources

| Repo | Language | What it contributes | License |
|---|---|---|---|
| [FozerG/WinYandexMusicRPC](https://github.com/FozerG/WinYandexMusicRPC) | Python | End-user app ("now playing → Discord"). Does **not** implement the socket itself; it delegates to the `yandex_music` SDK (`ymrpc/ynison.py` imports `YnisonClientAsync`). Contributes: metadata enrichment pattern, cover-URL construction, position extrapolation, short-session polling pattern. | **MIT** (© 2023 FozerG) — `/LICENSE` of that repo |
| [MarshalX/yandex-music-api](https://github.com/MarshalX/yandex-music-api) (transitive dep of the Python app; FozerG pins `git+...@0fa54f2d32084a9e461bce41890d1c9ab70d91aa` in `requirements.txt`) | Python | The **de-facto canonical** Ynison implementation: handshake headers, subprotocol construction, frame models (betterproto), keep-alive, reconnect policy, error frames. | **LGPL-3.0** (verified: `LICENSE` = "GNU LESSER GENERAL PUBLIC LICENSE Version 3"). Note for Rust: clean-room concerns are minor for a protocol re-implementation, but credit it. |
| [thedvxch/yandex-music](https://github.com/thedvxch/yandex-music) | TypeScript | Independent Ynison implementation: `src/ynison/client.ts`, `src/ynison/messages.ts`, `src/ynison/realtime.ts`. Contributes: exact initial-request JSON (betterproto `to_json` shape), error-`go-away` handling, stale-socket watchdog, `liveProgressMs`. | **MIT** (© 2026 dvxch) |

Key files read:

- `https://raw.githubusercontent.com/FozerG/WinYandexMusicRPC/main/ymrpc/ynison.py`
- `https://raw.githubusercontent.com/FozerG/WinYandexMusicRPC/main/ymrpc/yandex.py` (cover URL + metadata)
- `https://raw.githubusercontent.com/MarshalX/yandex-music-api/main/yandex_music/ynison/_base.py` (handshake, errors, ping)
- `https://raw.githubusercontent.com/MarshalX/yandex-music-api/main/yandex_music/ynison/_transport.py` (WS transport)
- `https://raw.githubusercontent.com/MarshalX/yandex-music-api/main/yandex_music/ynison/client.py` / `client_async.py`
- `https://raw.githubusercontent.com/MarshalX/yandex-music-api/main/yandex_music/ynison/messages.py` (request builders)
- `https://raw.githubusercontent.com/MarshalX/yandex-music-api/main/yandex_music/ynison/utils.py` (progress math)
- `https://raw.githubusercontent.com/MarshalX/yandex-music-api/main/yandex_music/ynison/models/ynison_state.py` (full protobuf model)
- `https://raw.githubusercontent.com/MarshalX/yandex-music-api/main/yandex_music/ynison/models/ynison_redirect.py`
- `https://raw.githubusercontent.com/MarshalX/yandex-music-api/main/tests/ynison_fake_server.py` (concrete frame examples)
- `https://raw.githubusercontent.com/thedvxch/yandex-music/main/src/ynison/client.ts`
- `https://raw.githubusercontent.com/thedvxch/yandex-music/main/src/ynison/messages.ts`
- `https://raw.githubusercontent.com/thedvxch/yandex-music/main/src/ynison/realtime.ts`
- `https://raw.githubusercontent.com/thedvxch/yandex-music/main/src/request.ts` (HTTP headers for metadata API)

## 1. Endpoints and handshake

The protocol is **JSON over normal WebSocket** (not binary protobuf on the wire;
it is a betterproto JSON projection of a gRPC-like service). Two-step handshake:

### Step 1 — Redirect service

```
wss://ynison.music.yandex.ru/redirector.YnisonRedirectService/GetRedirectToYnison
```

(`_base.py`: `_BASE_URL = 'wss://ynison.music.yandex.ru'`,
`_REDIRECT_SERVICE = 'redirector.YnisonRedirectService/GetRedirectToYnison'`; same
constants in `src/ynison/client.ts`.)

### Step 2 — State service

```
wss://{host-from-redirect}/ynison_state.YnisonStateService/PutYnisonState
```

`{host}` is the `host` string returned by the redirect frame (a dedicated player
host such as `ynison-…-mobile.yandex.net`, typically including its own port
443; both references blind-c interpolate it: `wss://{host}/{path}`).

### Handshake headers (exactly, from `_base.py::_get_headers()` and `client.ts::headers()`)

```
Origin: https://music.yandex.ru        # both sources mark it as required ("именно", "важно")
Authorization: OAuth <oauth-token>     # both sources mark it as required
```

- **No User-Agent** is set for the WebSocket handshake by either implementation.
- **No `X-Yandex-Music-Client`** header is sent on the WebSocket. That header
  belongs to the *HTTP metadata API* only (see §6): `X-Yandex-Music-Client:
  YandexMusicAndroid/24023621` (TS `src/request.ts`).

### Subprotocols (this is where the custom routing info lives)

Both implementations pass three `Sec-WebSocket-Protocol` values, in order:

```
Bearer
v2
<urlencoded device-info JSON>
```

From `_base.py::_get_device_info()` / `_get_subprotocols()` and `client.ts`:

```json
{
  "Ynison-Device-Id": "810eb13dbcd1",
  "Ynison-Device-Info": "{\"app_name\":\"Node.js SDK\",\"type\":\"1\"}"
}
```

- The **`Ynison-Device-Info` value is itself a JSON string** ("да, они реально
  тут хотят вложенный json.dumps иначе не работает" — comment in `_base.py`).
- When connecting to the **state** service, four keys are included:

```json
{
  "Ynison-Device-Id": "810eb13dbcd1",
  "Ynison-Device-Info": "{\"app_name\":\"yandex-music\",\"type\":\"1\"}",
  "Ynison-Redirect-Ticket": "<redirect_ticket from step 1>",
  "Ynison-Session-Id": "1700000000123"
}
```

- The whole object is JSON-serialized and **percent-encoded** (`urllib.parse.quote`
  / `encodeURIComponent`) as the third subprotocol token.
- `app_name` is the device *title* string shown to other devices in the Python
  SDK (default `"Python SDK"`; TS default `"Node.js SDK"`); `type` is the device
  class for the header: `"1"` = web (also used by FozerG), `"2"` = android (TS
  `ANDROID_DEVICE_INFO` uses `headerType: '2'`).

### Device/session ids

- `device_id`: any hex string; formats differ per reference and all work —
  random 8 bytes hex (TS, `randomUUID`-free 16 hex chars), 12 hex chars derived
  from `sha256(seed)[:12]` (Python SDK, seeded by token so restarts reuse one
  identity), `uuid5(...).hex` (FozerG). **Requirement:** a *stable* id per
  observer instance. Two live connections with the same id: the newer one
  **displaces** the older one (server sends an error frame, code `400090001`,
  `go-away 3600s`). Reusing a fresh random id on every reconnect makes the
  server drop you from the broadcast fan-out (documented in TS `realtime.ts`).
- `rid`: a UUID v4 string generated per request (`generateRequestId()` in TS,
  `str(uuid4())` in Python SDK).

## 2. Redirect frame (server → client, only frame on sockets 1)

One text frame, then the server closes the socket (close the client side in
parallel). Field names (betterproto JSON; server uses camelCase, parsers in
references accept both casings):

```json
{
  "host": "ynison-XXXXXXXX.mobile.yandex.net",
  "redirectTicket": "<opaque-token>",
  "sessionId": "1700000000123",
  "keepAliveParams": {
    "keepAliveTimeSeconds": "20",
    "keepAliveTimeoutSeconds": "20"
  }
}
```

Proto names (`ynison_redirect.py`): `host` (string, 1), `redirect_ticket`
(string, 2), `session_id` (**int64** → JSON string, 3), `keep_alive_params`
(message, 4) with `keep_alive_time_seconds` / `keep_alive_timeout_seconds`
(int32). `redirectTicket` authorizes the state connection (checked **before**
auth, anti-DDoS); send it back in the `Ynison-Redirect-Ticket` subprotocol JSON
and it is also used for the `Ynison-Session-Id`.

## 3. First client message on the state socket (initial "hello")

Sent **immediately after the state WS opens** (Python SDK in `_serve`, TS in
`on('open')`). It registers this listener as a *remote control (not player)*
with an empty queue. Exact shape — TS `buildUpdateFullStateRequest` in
`src/ynison/messages.ts` (betterproto `to_json` shape: camelCase, int64 as
strings, defaults omitted) with the richer Python-SDK fields added in comments:

```json
{
  "updateFullState": {
    "playerState": {
      "status": {
        "paused": true,
        "playbackSpeed": 1,
        "version": { "deviceId": "810eb13dbcd1" }
      },
      "playerQueue": {
        "entityType": "VARIOUS",
        "currentPlayableIndex": -1,
        "options": { "repeatMode": "NONE" },
        "version": { "deviceId": "810eb13dbcd1" },
        "fromOptional": ""
      }
    },
    "device": {
      "info": {
        "deviceId": "810eb13dbcd1",
        "title": "Node.js SDK",
        "type": "WEB",
        "appName": "yandex-music"
      },
      "capabilities": { "canBeRemoteController": true },
      "volumeInfo": {}
    }
  },
  "rid": "7c9e6679-7425-40de-944b-e07fc1f90ae7"
}
```

Python-SDK equivalents you may add (from `messages.build_full_state_request`):
`capabilities.can_be_player: false`, `capabilities.volume_granularity: 0`,
`volumeInfo: {"volume": 0}`, `is_currently_active: false`,
`playerQueue.entity_context: "BASED_ON_ENTITY_BY_DEFAULT"`, top-level
`playerActionTimestampMs` (int64 ms epoch, `"0"` allowed) and
`activityInterceptionType: "DO_NOT_INTERCEPT_BY_DEFAULT"` (default → omitted).
Omitted fields mean their protobuf defaults (0 / "" / false / enum 0); a
`version` may carry `"version"` (random int64, JSON string) and `"timestampMs"`
(ms epoch) — `timestamp_ms` is documented as *diagnostic, not business logic*.

Casing works both ways: the Python SDK serializes via betterproto `to_dict()`
(snake_case keys: `update_full_state`, `playable_id`, …) and the TS via
camelCase; the server accepts both. What the **server sends back** is the
betterproto `to_json` shape: **camelCase keys, int64 as strings, default values
omitted** (documented in `src/ynison/messages.ts` header comment). Deserialize
tolerantly (accept both, defaults for missing fields) exactly like
TS `parseStateFrame` (`toNum` / `field(obj, 'snake', 'camel')`).

Identity override presenting as the **Android client** makes the server expose
richer state (TS `ANDROID_DEVICE_INFO`):

```json
{ "type": "ANDROID", "headerType": "2", "appName": "ru.yandex.music",
  "appVersion": "2026.05.3", "title": "yandex-music" }
```

(use `"type": "ANDROID"` in `device.info`, `"2"` in the `Ynison-Device-Info`
subprotocol JSON, and `"appVersion": "2026.05.3"` in `device.info`).

## 4. Server frames

### 4.1 State snapshot — `PutYnisonStateResponse`

Full message model from `models/ynison_state.py`; an annotated skeleton with
example values (anonymized):

```json
{
  "playerState": {
    "status": {
      "progressMs": "38250",
      "durationMs": "253890",
      "playbackSpeed": 1,
      "version": {
        "deviceId": "f3-player-device01",
        "version": "1048576000000000000",
        "timestampMs": "1760700000000"
      }
    },
    "playerQueue": {
      "entityId": "1234567",
      "entityType": "PLAYLIST",
      "currentPlayableIndex": 0,
      "playableList": [
        {
          "playableId": "94015473",
          "albumIdOptional": "11122233",
          "playableType": "TRACK",
          "title": "Track Title",
          "coverUrlOptional": "https://avatars.yandex.net/get-music-content/NNNN/…/%%"
        }
      ],
      "options": { "repeatMode": "NONE" },
      "version": { "deviceId": "f3-player-device01", "version": "…" }
    }
  },
  "devices": [
    {
      "info": {
        "deviceId": "f3-player-device01",
        "title": "Yandex Music Desktop",
        "type": "WEB",
        "appName": "YandexMusic",
        "appVersion": "5.x"
      },
      "volume": 0.5,
      "capabilities": { "canBePlayer": true, "canBeRemoteController": true, "volumeGranularity": 16 },
      "session": { "createdAtMs": "…" },
      "isOffline": false,
      "volumeInfo": { "volume": 0.5, "version": { "deviceId": "…", "version": "…" } }
    }
  ],
  "activeDeviceIdOptional": "f3-player-device01",
  "timestampMs": "1760700001000",
  "rid": "42c11c11-f4b8-4c8a-a1e5-3f0f1c2b3d4e"
}
```

Notes per field (all documented in `ynison_state.py` docstrings):

- `status.paused` is **omitted when `false`** (default-value elision). An
  absent `paused` ⇒ playing. Same for `playbackSpeed` (absent ⇒ `1`), but note
  `playback_speed == 0` is a real value (occurs for infinite streams) — TS test
  `playback_speed: 0 is preserved, not coerced to 1`.
- `status.progressMs`, `status.durationMs` — **milliseconds** (int64, string in
  JSON). For infinite queues (radio/neuro) both are `"0"`.
- `status.version.timestampMs` — ms since Unix epoch, "the time of the last
  status change" (diagnostic field; the references still use it to extrapolate
  position, see §7).
- Response-level `timestampMs` — time the server created the frame (int64 ms).
- `playerQueue.currentPlayableIndex` — index into `playableList`; `-1` means
  nothing selected/empty queue.
- `shuffleOptional.playableIndices` — present only when shuffle is on; gives
  the actual playback order.
- `activeDeviceIdOptional` — id of the playing device into `devices[]`
  (absent/empty when nothing is playing → treat as "idle").
- Device type enum (`DeviceType`): `UNSPECIFIED=0, WEB=1, ANDROID=2, IOS=3,
  SMART_SPEAKER=4, WEB_TV=5, ANDROID_TV=6, APPLE_TV=7, ANDROID_WEAR=8,
  WEB_DESKTOP=9`.
- Playable type enum (`PlayablePlayableType`): `UNSPECIFIED=0, TRACK=1,
  LOCAL_TRACK=2, INFINITE=3, VIDEO_CLIP=4`. FozerG skips anything with
  `playableType != 1` (i.e. only real `TRACK`s are shown).

### 4.2 What is *not* in the frame: track metadata

Ynison frames carry the **track id** (`playableId`), sometimes `title` /
`coverUrlOptional` (depends on the reported device client), but **never
artists/album/duration reliably**. `Playable.track_info` oneof is only
recommendation bookkeeping (`track_source_key`, `batch_id_optional`) — **not**
title/artists. Both references therefore enrich over the normal HTTP API:

- FozerG (`ymrpc/ynison.py`): `Client(token=token).tracks([track_id])` →
  `https://api.music.yandex.ru/tracks?ids=<trackId>` with `Authorization: OAuth
  <token>`; caches by track id.
- TS: `resolveTrack` callback → library `tracks()` (same endpoint).

Cover URL template (FozerG `ymrpc/yandex.py`, applied to Track.cover_uri /
og_image):

```
https:// + cover_uri.replace("%%", "400x400")
```

i.e. the raw template looks like
`avatars.yandex.net/get-music-content/<sid>/<hash>/%%` and the `%%`
placeholder is substituted with a size literal (`400x400`, `200x200`, `50x50`, …).

Track URL: `https://music.yandex.ru/album/{albumId}/track/{trackId}`
(`split(":")[0]` on the id to drop the region suffix).

### 4.3 Error frames (server → client)

Modelled in `_base.py::parse_server_error` and TS `parseErrorFrame`. Any frame
with an `error` key:

```json
{
  "error": {
    "grpcCode": 16,
    "httpCode": 401,
    "message": "UNAUTHENTICATED: token expired",
    "details": {
      "ynison-error-code": "400160000",
      "ynison-backoff-millis": "0:1000:5000:30000",
      "ynison-go-away-for-seconds": "3600"
    }
  }
}
```

- Details may appear under `details` or `extra_headers`.
- `grpcCode` 16 (UNAUTHENTICATED) / 7 (PERMISSION_DENIED) or `httpCode`
  401/403 ⇒ auth failure — **terminal** (wrong/expired token).
- `ynison-error-code == "400090001"` ⇒ your `deviceId` was displaced by another
  client — **terminal** for that id.
- `ynison-go-away-for-seconds` ⇒ do not reconnect for N seconds (can be 3600;
  TS caps this at `goAwayMaxMs`, default 60 s).
- `ynison-backoff-millis` — colon list of reconnect delays, e.g.
  `"0:1000:5000:30000"` (this is also the SDK's built-in default ladder,
  `_DEFAULT_BACKOFF_MS`, with 0–250 ms jitter added per attempt).

## 5. Keep-alive

1. **Protocol-level WebSocket ping/pong, not an app-level message.**
   - Interval/timeout come from the redirect frame's `keepAliveParams`
     (`_base.py::_ping_params`: interval = `keepAliveTimeSeconds`, timeout =
     `keepAliveTimeoutSeconds`; fall back to **20 s / 20 s**
     (`_DEFAULT_PING_INTERVAL/_DEFAULT_PING_TIMEOUT`) if absent). TS uses a
     fixed `KEEPALIVE_PING_INTERVAL_MS = 20_000`.
   - Python `websockets` gets `ping_interval`/`ping_timeout` directly; TS calls
     `ws.ping()` on a `setInterval`.
2. **No TLS post-handshake ping workaround exists in any of the references.**
   Neither `_transport.py` (plain `websockets` connect, no custom
   `ssl_context`) nor the TS `ws` usage touches TLS layers, and there is no
   "yandex flag" header anywhere in the code. If a Rust WS library complains
   about Yandex TLS post-handshake traffic, that would be a local issue; the
   references need nothing special (they do set
   `max_size = 64 * 1024 * 1024` for the receive buffer — full-state frames of
   huge queues can exceed websockets' default 1 MiB).
3. Liveness guidance (TS `realtime.ts`): Ynison pushes state frames **only on
   changes**; a paused/idle player is legitimately silent for minutes. Track
   transport liveness via "any inbound frame/ping/pong" (`lastActivityAt`), and
   force reconnect after ~120 s of total socket silence, not frame silence —
   tearing down on frame silence causes reconnect storms, rate-limiting, and
   host rebalancing. ("Yandex TLS post-handshake ping" is not used/relevant.)

## 6. Token & metadata-API headers

- Ynison auth: `Authorization: OAuth <token>` in the WS handshake. The token is
  the regular Yandex Music OAuth token (obtainable via
  `https://oauth.yandex.ru/authorize?response_type=token&client_id=23cabbbdc6cd418abb4b39c32c41195d`
  — used by FozerG `ymrpc/constants.py`).
- HTTP metadata enrichment (`api.music.yandex.ru`), TS `src/request.ts`:

```
Authorization: OAuth <token>
X-Yandex-Music-Client: YandexMusicAndroid/24023621
User-Agent: dvxch-yandex-music        (arbitrary; both samples send one for HTTP only)
```

## 7. Update semantics and position computation

- Updates are **full snapshots**, not deltas: every state frame carries the
  complete `PlayerState` + `devices[]`. Handlers can just replace the last
  snapshot (TS keeps `lastState`; Python `latest_state`).
- The server pushes when: any connected device changes something, the device
  list changes, or as a direct reply to a client's own `PutYnisonStateRequest`.
- There are no periodic progress frames; a playing track is silent between
  state changes.
- Position at time *now* (Python `utils.get_current_progress_ms`, TS
  `liveProgressMs`, FozerG — identical math):

```
if paused or timestamp_ms == 0:      position = progress_ms
else:
    elapsed_ms = now_ms - status.version.timestamp_ms
    position = progress_ms + elapsed_ms * playback_speed   # speed 0 => 1
    clamp to [0, duration_ms] when duration_ms > 0
```

  Guard against insane server clocks: FozerG ignores `elapsed > 86400 s`.
  `playbackSpeed` is a double (`0` possible for infinite → treat as 1 for math,
  TS test keeps 0 distinct from missing).
- Track change detection: compare
  `playableList[currentPlayableIndex].playableId` of the current snapshot with
  the previous one (TS `handleState`). State types: playing/paused derived from
  `status.paused`; "stopped/idle" shows up as: no `activeDeviceIdOptional`, or
  `currentPlayableIndex` outside `[0, len(playableList))`, or empty
  `playableList` (FozerG treats all of these as idle).

## 8. Reconnection policy (robustness facts from the references)

- Always redo the **redirect** step on every reconnect (the "host" is per
  session).
- Backoff ladder from server (`ynison-backoff-millis`) or default
  `[0, 1000, 5000, 30000]` ms + 0–250 ms jitter (Python SDK); TS uses
  exponential `base * 2^attempt` capped at 30 s and honours go-away capped at
  60 s.
- Terminal errors (do **not** auto-reconnect): unauthorized (grpc 16 / http
  401/403), device displaced (`400090001`), any frame with
  `ynison-go-away-for-seconds` (or reconnect only after the go-away interval).
- One `device_id` per live connection — reconnect with the **same** id or you
  vanish from the broadcast.
- FozerG's alternative pattern for a status app: short polled sessions
  (connect → wait first state (8 s timeout) → disconnect → sleep 3 s), which
  avoids long-lived displacement management entirely.
- Scope discipline: a snapshot frame's fields suffice for "now playing"
  indicator: `get_current_playable(state)` +
  `get_active_device(state)` (`state.active_device_id_optional`).

## 9. Rust implementation notes (protocol mapping only)

- Handshake needs custom `Sec-WebSocket-Protocol` tokens, `Origin`,
  `Authorization` — e.g. `tungstenite::connect` variant accepting a custom
  request works (`tungstenite` supports subprotocols + arbitrary headers).
  The device-info subprotocol token must be percent-encoded the way
  `urllib.parse.quote`/`encodeURIComponent` do it.
- Message frames are **text** WebSocket frames with nicer JSON; ignore binary.
- Ping: emit protocol-level `Ping` every `keepAliveTimeSeconds` (default 20 s),
  drop the connection when no `Pong` within `keepAliveTimeoutSeconds`
  (default 20 s); a pong-less interval ⇒ reconnect via the redirect step.
- int64 JSON values are strings — deserialize as `u64`/`i64` from string *or*
  number.
- **Production frame casing is `snake_case`** (like the redirect frame; the
  canonical fake-server fixtures are camelCase). Tolerant decoding MUST accept
  both name forms for every field.
- **Production `PutYnisonStateResponse` nests the queue INSIDE `player_state`**
  (`player_state: {status: …, player_queue: …}`), it is not a top-level
  sibling as in the canonical test fixtures. Accept both layouts.
- Absent fields = protobuf defaults: `paused=false ⇒ playing`,
  `playbackSpeed=1`, `currentPlayableIndex=-1`.
- Receive buffer: allow multi-MiB frames (full-state snapshots of large queues).

## 10. Licenses

- **FozerG/WinYandexMusicRPC** — MIT License, `Copyright (c) 2023 FozerG`
  (full text at the repo's `LICENSE`).
- **thedvxch/yandex-music** — MIT License, `Copyright (c) 2026 dvxch`.
- **MarshalX/yandex-music-api** (the SDK FozerG's app actually uses for the
  socket protocol) — **LGPL-3.0** (`LICENSE` = "GNU LESSER GENERAL PUBLIC
  LICENSE Version 3, 29 June 2007"). If this Rust client is proprietary, treat
  MarshalX's code as reference-only (documented facts above, not copied code);
  the TS repo (MIT) plus this document suffice as the clean-room basis.
