# Research: Discord Rich Presence over Local IPC (Windows / Rust / Tauri, no bot token)

Date of research: 2026-10-17
All sources fetched and verified during this session (links inline). Nothing below is from memory alone; items marked **community-known** are specifically identified as such because the official docs no longer (or never did) cover them.

---

## 1. The `discord-rich-presence` crate

**Repo:** https://github.com/vionya/discord-rich-presence (moved from the original `sardonicism-04`/`nickofolas` repos created in 2021)
**crates.io:** https://crates.io/crates/discord-rich-presence

Verified via the crates.io API on 2026-10-17:

| Item | Value |
|---|---|
| Current version | **1.1.0**, published **2026-01-26** (`newest_version`, `max_version`, `default_version` = 1.1.0) |
| Version 1.0.0 | published 2025-09-02 (breaking release from 0.2.5) |
| License | **MIT** (confirmed both via crates.io `license` field and the GitHub `license` object) |
| Downloads | 658,746 total, ~279k in the last 90 days |
| Repo status | **not archived, not a fork**; last push **2026-06-28**; repo `updated_at` 2026-10-05; 156 stars, 9 open issues |
| Maintenance | Actively maintained: two releases in the last 12 months (1.0.0 in 2025-09, 1.1.0 in 2026-01). The 1.x line modernized the codebase to Rust 2021-ish hygiene (`#[must_use]`, `Cow<'a, str>`, `const fn` builders, `SendActivity` additions) |
| Platform support | Windows + Unix (named pipe vs. unix domain socket), single platform-agnostic API |
| Dependencies (per source) | `serde` + `serde_derive` (Serialize on activity structs), `serde_json`, `serde_repr`, `uuid` |

Verdict: the crate is **adequate and maintained**; no alternative is strictly required.

### 1.1 Exact API (read from `src/discord_ipc.rs`, `src/activity.rs`, `src/ipc_windows.rs` on the `main` branch)

The crate exposes a trait `DiscordIpc` plus a concrete `DiscordIpcClient` (Windows implementation opens `\\?\pipe\discord-ipc-{0..9}`; Unix implementation probes the usual temp dirs).

#### Minimal usage (README example)

```rust
use discord_rich_presence::{activity, DiscordIpc, DiscordIpcClient};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = DiscordIpcClient::new("<some application ID>");
    client.connect()?;
    client.set_activity(activity::Activity::new().state("foo").details("bar"))?;
    client.close()?;
    Ok(())
}
```

Note: `DiscordIpcClient::new(client_id: impl AsRef<str>)` does **not** return `Result` (Windows/Unix implementations confirmed); it stores the client id and `connect()` does the work. The trait methods need `use discord_rich_presence::DiscordIpc` in scope.

#### Connecting / reconnecting / closing

```rust
// trait DiscordIpc:
client.connect()?;          // connect_ipc() + send_handshake() { {"v":1,"client_id":"..."} }
client.reconnect()?;        // close() then connect_ipc() + send_handshake()
client.close()?;            // sends opcode 2 (CLOSE) with json!({}), drops the pipe handle
```

`connect_ipc()` on Windows tries `\\?\pipe\discord-ipc-0` … `-9` in order and returns `Error::IPCConnectionFailed` if none are open. `send_handshake()` sends opcode `0` with `{"v": 1, "client_id": "<id>"}` and then blocks on `recv()` for the READY frame. Note there is a `TODO` in the crate: the handshake rejection is **not** turned into an `Err` — but Discord does not require any auth handshake for set-activity over IPC (see §2/§4).

#### Setting / clearing an activity

```rust
// internally sends opcode 1 (FRAME) with:
// { "cmd": "SET_ACTIVITY",
//   "args": { "pid": <process id>, "activity": <Activity> },
//   "nonce": "<uuid v4>" }

client.set_activity(activity::Activity::new().state("In Competitive Match"))?;
client.clear_activity()?;   // same envelope, but "activity": null
```

Both `set_activity` and `clear_activity` are framework-level defaults on the `DiscordIpc` trait — provided by the crate, not the platform backends.

#### `activity::Activity` builder surface (all builders consume `self`, return `Self`, are `Option`-backed with `skip_serializing_if = "Option::is_none"` — omitted fields are not serialized)

| Builder method | Field / notes |
|---|---|
| `.name(S)` | displayed app name override (v1.x addition) |
| `.state(S)` | second line text |
| `.state_url(S)` | click URL for state text (federated-presence addition) |
| `.details(S)` | first line text |
| `.details_url(S)` | click URL for details text |
| `.timestamps(Timestamps)` | see below |
| `.party(Party)` | `Party::new().id(str).size([cur, max])` → `"size": [i32;2]` |
| `.assets(Assets)` | see below |
| `.secrets(Secrets)` | `join`, `spectate`, `match` (`r#match`) |
| `.buttons(Vec<Button>)` | `Button::new(label, url)`; an **empty Vec is silently skipped** at build time (never serialized) |
| `.activity_type(ActivityType)` | enum: `Playing = 0`, `Listening = 2`, `Watching = 3`, `Custom = 4`, `Competing = 5` (`Streaming = 1` is **missing** from the enum — see §4 caveat) |
| `.status_display_type(StatusDisplayType)` | `Name = 0`, `State = 1`, `Details = 2` (what appears in the member-list status text) |

- `Timestamps::new().start(i64).end(i64)` — Unix time **in milliseconds** (documented in the struct docs; the Discord Social SDK docs describe seconds, gateway/RPC treat them as ms — verify empirically if precision drifts).
- Assets: `Assets::new().large_image(S).large_text(S).large_url(S).small_image(S).small_text(S).small_url(S)`. `large_url`/`small_url` (click-through URL on the image) is a newer Discord field, present in current docs and in the crate. There is also a community-updated `invite_cover_image` field in Discord's docs which the crate does **not** builder-expose (irrelevant for a music activity).
- The structs are plain private-field `#[derive(Serialize)]` types: **all** fields serialize as their private Rust field names, so the JSON shape is exactly the documented Discord activity shape. Everything is a typed `Option<Cow<str>>` etc., which means payloads are constrained to the implemented fields.

#### 1.2 Does the payload allow arbitrary nested JSON (e.g. raw `assets` or `mp:external/...` keys)?

**Not directly.** There is no `#[serde(flatten)] extra: BTreeMap<String, Value>` or `Json<Value>` passthrough on `Activity`. However:

1. You can pass **any string** into `large_image` / `small_image` (they are plain `Cow<'a, str>`), which includes a full `https://` URL or a community-style `mp:external/<hash>/<filename>` key — string-valued fields are unconstrained at the crate level, so the media-proxy scheme (§3) is fully usable.
2. If you ever need fields the crate hasn't got (e.g. `instance`, `url`, `emoji`, `invite_cover_image`), you can bypass the builder entirely: `DiscordIpc::send(payload: serde_json::Value, opcode: u8)` is a public trait method, so you can hand-roll the `{"cmd": "SET_ACTIVITY", "args": {...}, "nonce": "..."}` envelope with a `json!({...})` literal and send it yourself. That makes the crate flexible enough for arbitrary nested JSON with ~5 lines of code.
3. Fields the crate cannot currently express: `url` (Streaming type), `emoji`, `instance`, `name` override is supported but `activity::Activity` has no `#[serde(rename)]` issues — `type` is correctly renamed (`#[serde(rename = "type")]`).

### 1.3 Error handling

`discord_rich_presence::error::Error` variants referenced by the implementations include at least: `IPCConnectionFailed`, `NotConnected`, `WriteError`, `ReadError`, `FlushError`, `RecvUtf8Response`, `JsonParseResponse`. Everything returns `Result<T, Error>`.

---

## 2. Dynamic image support: `large_image` external URLs and the `mp:` scheme

### What is **officially documented** now (verified against `discord-api-docs@main`, 2026-10)

From `developers/events/gateway-events.mdx` → *Activity Object → Activity Assets → Activity Asset Image* (this is the doc page `SET_ACTIVITY` points to via its `activity` field):

> *Activity asset images are arbitrary strings which usually contain snowflake IDs or prefixed image IDs. Treat data within this field carefully, as it is user-specifiable and not sanitized.*
>
> *To use an external image via media proxy, specify the URL as the field's value when sending. You will only receive the `mp:` prefix via the gateway.*
>
> | Type | Format | Image URL |
> |---|---|---|
> | Application Asset | `{application_asset_id}` | `https://cdn.discordapp.com/app-assets/{application_id}/{application_asset_id}.png` (via Image Formatting) |
> | Media Proxy Image | `mp:{image_id}` | `https://media.discordapp.net/{image_id}` |
>
> **Uploaded assets** (Application Asset type) support PNG, JPEG, and WebP. Animated images are not supported for uploaded assets.
> **External URL assets** (Media Proxy Image type) support any publicly accessible image URL, including GIF, animated WebP, and AVIF.

And from `developers/rich-presence/overview.mdx` / the [Social SDK "Setting Rich Presence" guide](https://discord.com/developers/docs/discord-social-sdk/development-guides/setting-rich-presence):

> *Up to 300 custom assets can be added to your app… If you need more than 300 custom assets or want to use images stored somewhere else, you can also **specify an external URL** as long it still has the proper dimensions and size. Unlike uploaded assets, external URLs also support GIF, animated WebP, and AVIF.*
> *When uploading Rich Presence assets, the asset keys will automatically be changed to lowercase.*

**Practical consequence (officially sanctioned, current):** for dynamic art (e.g. album covers), you can simply PUT a full `https://.../cover.jpg` URL into `assets.large_image` in every `SET_ACTIVITY` call. No upload, no app-panel asset registration, no bot token. The docs explicitly state the URL is only a hint at send time; anything roundtripping back to you (or other clients via the gateway) arrives as an `mp:{image_id}` key, resolvable at `https://media.discordapp.net/{image_id}`.

Also officially documented in the same guide: **Rich Presence can be set without authentication** (Rich Presence Without Authentication section): "Unlike most other features of the Discord Social SDK, Rich Presence can be set without authentication… Set your application ID… no Connect() call needed". For local IPC clients this matches 15 years of `discord-ipc` behavior: only the client ID from the handshake is needed; no OAuth2 `rpc` scope token, no RPC approval, no bot token. (Caveat: the general RPC doc still *says* commands require authentication with error 4006 — this refers to the full RPC command set (GET_GUILD, SUBSCRIBE, etc.). Client-less `SET_ACTIVITY` over IPC works without an access token; Discord's own Social SDK documents the tokenless path.)

### What is **not officially documented** (community-known)

- The exact intermediate key format **`mp:external/<hash>/<filename>`** (the "Spotify-style" external asset URL). Official docs only teach the abstract `mp:{image_id}` scheme and the *send a plain URL* trick; the `external/<sha-ish hash>/<encoded path>` concrete form visible in real traffic (and in Spotify's own presence) is reverse-engineered. It is widely reproduced in third-party projects (e.g. the `discord-rpc-selfbot` project's asset key guide lists `mp:external/<hash>`, `spotify:<image_id>`, `twitch:<username>`, `youtube:<video_id>` as recognized key prefixes).
- Other undocumented legacy prefixes in circulation: `spotify:<image_id>`, `twitch:<username>` (IETF-style legacy link).

**Recommendation for this project:** never construct `mp:external/...` strings by hand. Send the raw `https://` cover URL in `large_image` every time the track changes; let Discord's client do the proxying/registration (that is the officially supported path as of 2026). Session-caching behavior (the client resolves/caches the external asset for the session) is community-observed economics, not documented — so re-sending the same URL on each track switch is cheap and safe.

### "Presence metadata / image resolvers"

The docs do not use the phrase "image resolvers" in the current RPC/Rich-Presence pages. What exists today:

- The **Discord client itself** is the resolver: your IPC `SET_ACTIVITY` carries the URL; the desktop client performs the external-asset registration and turns it into an `mp:` asset id; other clients receive that id and dereference it against `media.discordapp.net`.
- There is **no public API** to pre-register/metadata-ize arbitrary external asset images; the app-panel "Rich Presence → Art Assets" upload (≤300 assets, keys lowercased automatically, PNG/JPEG/WebP only) is the only pre-registered-asset mechanism, and it is optional.
- So: for dynamic album art you **cannot rely on metadata pre-registration** and must rely on the per-send URL mechanism.

### Rate / behavior limits verified

| Limit | Value | Source status |
|---|---|---|
| Presence updates cadence | **`Clients may only update their game status 5 times per 20 seconds.`** (Gateway warning, verified in `developers/events/gateway-events.mdx`) | **Official** — throttle to ≤5 requests / 20 s; a music app naturally emits at most one per track change, comfortable. |
| `details` / `state` length | **No numeric cap in current official docs**; only best-practices advice ("keep it short", "stay on one line") | **Official doc = no cap published.** Community/legacy knowledge (pre-2022 game-SDK docs and most wrapper libraries) cites **128 bytes** each for `details`/`state`/`large_text`/`small_text`. Treat 128 chars as a de-facto practical cap, not a documented guarantee. |
| Buttons | `max 2`; `label` **1–32 chars**; `url` **1–512 chars** | **Official** (gateway Activity Buttons table) |
| Enforced `type` over `SET_ACTIVITY` | *"When using SET_ACTIVITY, the activity object is limited to a type of Playing (0), Listening (2), Watching (3), or Competing (5)."* i.e., **Streaming (1) and Custom (4) are excluded** | **Official** (RPC docs Info block). The `discord-rich-presence` enum oddity is that it also exposes `Custom = 4` which the RPC docs say is unsupported for SET_ACTIVITY (useful only for unofficial clients). Use `Listening = 2` for a music app. |

---

## 3. IPC transport & handshake (Windows)

Verified against `developers/topics/rpc.mdx` on `discord-api-docs@main`:

**IPC path / socket name (Windows):** `\\?\pipe\discord-ipc-{n}`
(Linux/macOS order: `${XDG_RUNTIME_DIR}/discord-ipc-{n}`, `${TMPDIR}/discord-ipc-{n}`, `${TMP}/discord-ipc-{n}`, `${TEMP}/discord-ipc-{n}`, `/tmp/discord-ipc-{n}`.)

- `n` = the index the client picks, trying to find an open pipe; Discord runs at most a handful of sockets (`0..=9` is the widely-used probe range; the crate uses `0..10`, same thing). Community implementations all scan `-0` → `-9`. Docs do not fix `n` explicitly beyond `{n}`
- There are no official port/bindings tables for the pipe count; the scan-window is effectively community standard practice.

**Opcodes** (Frame header = little-endian native-endian u32 op, u32 length, then payload):

| Op | Name | Meaning |
|---|---|---|
| 0 | HANDSHAKE | initiate |
| 1 | FRAME | command / event frame |
| 2 | CLOSE | close |
| 3 | PING | keepalive probe |
| 4 | PONG | reply to PING |

**Handshake payload (JSON):** `{"v": 1, "client_id": "<app id>"}`
Verified example line in the docs (`[00 00 00 00]` `[2D 00 00 00]` then `{"v":1,"client_id":"123456789012345678"}`).

After the handshake, Discord replies with a `FRAME` carrying the `READY` event (this contains `v`, `config.cdn_host/api_endpoint/environment`, and the current user). This is the point most people use to confirm "Discord client is up". Subsequent frames are all op-1 with the standard `{cmd, args?, data?, evt?, nonce}` envelope.

**Cross-check with the crate source:** `discord-rpc`-sharing the same headers, `discord-rich-presence`'s `pack_unpack.rs` handles the 8-byte header exactly like this (op + len), and `DiscordIpcClient::connect_ipc` opens `\\?\pipe\discord-ipc-{i}` for `i in 0..=9` via `OpenOptions::new().access_mode(0x3)` (read+write) — this is the canonical Windows named-pipe "socket" open. So there is no need to implement the pipe layer by hand even if a fallback is ever wanted.

---

## 4. Verdict for the project

`discord-rich-presence` (v1.1.0, MIT, actively maintained as of 2026-01) is a **good fit** for the Tauri music-activity project:

- Officially documents unauthenticated Rich Presence over local IPC — exactly the "no bot token" requirement (no OAuth flow, no RPC approval, no token, just your Application ID from the developer portal).
- Supports `Listening` (type 2), clickable fields `state_url`/`details_url`, button labels/URLs, and media-proxy external images by URL string — all straightforwardly.
- Rate-limits are not a concern: track-change updates are naturally well under 5 / 20 s.
- The 128-byte de-facto cap on `details`/`state`/`large_text` should be respected defensively in case Discord keeps its undocumented truncation limit (libratings under 30 chars would still be safest on the *small profile popout* where line-wrap accentuates overflow).

### Alternatives (only needed if this crate ever falls behind)

| Crate | Version | License | Notes |
|---|---|---|---|
| `discord-presence` (by `jewlexx`) | **3.2.0** (published 2026-01-20) | **MIT** | Event-callback API (`Client::new(id)`, `.on_event(Event::Ready, …)`, `.start()`, `.set_activity(\|act\| act.state(…))`). Fork lineage back to Discord RPC Client. ~9,847 recent downloads (monthly-ish) — a smaller user base than `discord-rich-presence` (279k recent) but also actively maintained. |
| roll-your-own (custom `\\?\pipe\discord-ipc-N` client) | – | – | Feasible with `Windows::storage::OpenOptions`-equivalent `std::os::windows::fs::OpenOptions` + `access_mode(0x3)`; you already know the payload format from §1.1 and §3. Useful only if you need exotic activity fields (e.g., `Streaming`, `instance`) the maintained crates both omit. |

### Bottom-line caveats to carry forward

1. **Timestamps unit ambiguity (ms vs s):** `discord-rich-presence`'s doc comment says milliseconds for `Timestamps::start/end`; the Social SDK docs describe `time(nullptr)`-style seconds. Empirically test with a 1-minute window at first startup; pick whatever makes the progress bar render correctly in your own testing.
2. **Auth-while-testing:** the docs repeatedly warn that *unapproved* apps/authenticated RPC commands are restricted to approved-testers; set-activity-over-IPC is the documented exception (Rich Presence Without Authentication). Still, don't attempt the full RPC command set from this app — it will raise 4006.
3. **Streaming type doesn't apply** to `SET_ACTIVITY` (officially constrained to 0/2/3/5), so the `Listening` (2) choice is both the right vibe and the right technical value.
4. **No numeric guarantees** in current docs about `details`/`state` string length; keep them short + single-line per the official best-practices and treat 128 as the comfortable ceiling.
5. Don't hand-construct `mp:external/<hash>/<filename>` keys; send raw `https://` URLs as directed by the current official docs.
