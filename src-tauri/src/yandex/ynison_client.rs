//! Жизненный цикл WebSocket Ynison (ТЗ §5.2).
//!
//! Двухшаговый handshake по результатам исследования
//! `docs/research/ynison-protocol.md`:
//! 1. `wss://ynison.music.yandex.ru/redirector.YnisonRedirectService/GetRedirectToYnison`
//!    → один текстовый кадр `{host, redirectTicket, sessionId, keepAliveParams}`.
//! 2. `wss://{host}/ynison_state.YnisonStateService/PutYnisonState` — рабочий
//!    поток с subprotocols `Bearer`, `v2` и percent-encoded JSON метаданных
//!    устройства (Device-Id, Device-Info; на state-сокете — Redirect-Ticket
//!    и Session-Id).
//!
//! Сервер присылает полные снапшоты. Keep-alive — протокольные WS ping/pong.
//! Переподключение — exponential backoff с jitter (ТЗ §5.2).

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use tokio_util::sync::CancellationToken;

use crate::app_state::{AppSharedState, YnisonStatus};
use crate::config::persistence::load_or_create_device_id;
use crate::presence::backoff::Backoff;
use crate::yandex::errors::{AppError, AppResult, ErrorCode};
use crate::yandex::metadata::MetadataClient;
use crate::sources::PlaybackState;
use crate::yandex::protocol::{normalize, parse_frame, MAX_FRAME_BYTES};

/// Базовый URL redirect-сервиса.
pub const REDIRECT_URL: &str =
    "wss://ynison.music.yandex.ru/redirector.YnisonRedirectService/GetRedirectToYnison";

/// Таймаут handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
/// Дефолтный keep-alive период (перекрывается параметрами сервера).
const DEFAULT_KEEPALIVE_SECS: u64 = 20;
const MIN_KEEPALIVE_SECS: u64 = 5;
const MAX_KEEPALIVE_SECS: u64 = 300;

/// Параметры redirect, полученные с первого сокета.
///
/// Разбор — толерантный к casing (camelCase/snake_case) и к типам int64
/// (строка/число): промышленный сервер смешивает форму, а strict-разбор
/// раньше приводил к тому, что `redirectTicket` улетал серверу как `null` →
/// INVALID_ARGUMENT 400030003.
#[derive(Debug, Clone, Default)]
pub struct RedirectInfo {
    pub host: Option<String>,
    pub redirect_ticket: Option<String>,
    pub session_id: Option<String>,
    pub keep_alive_params: Option<serde_json::Value>,
}

/// Первое не-null значение по двум возможным именам поля.
fn field<'a>(json: &'a serde_json::Value, camel: &str, snake: &str) -> Option<&'a serde_json::Value> {
    let value = json.get(camel).filter(|v| !v.is_null());
    value.or_else(|| json.get(snake).filter(|v| !v.is_null()))
}

/// Строка или число (int64): оба представления допустимы протоколом.
fn field_str(json: &serde_json::Value, camel: &str, snake: &str) -> Option<String> {
    match field(json, camel, snake) {
        Some(serde_json::Value::String(text)) => Some(text.clone()),
        Some(number @ serde_json::Value::Number(_)) => Some(number.to_string()),
        _ => None,
    }
}

impl RedirectInfo {
    /// Разобрать redirect-кадр сервера толерантно.
    pub fn parse(json: &serde_json::Value) -> AppResult<Self> {
        Ok(Self {
            host: field_str(json, "host", "host"),
            redirect_ticket: field_str(json, "redirectTicket", "redirect_ticket"),
            session_id: field_str(json, "sessionId", "session_id"),
            keep_alive_params: field(json, "keepAliveParams", "keep_alive_params").cloned(),
        })
    }

    fn param_u64(obj: &serde_json::Map<String, serde_json::Value>, keys: &[&str]) -> Option<u64> {
        for key in keys {
            let raw = match obj.get(*key) {
                Some(raw) => raw,
                None => continue,
            };
            if let Some(value) = raw.as_u64() {
                return Some(value);
            }
            if let Some(text) = raw.as_str() {
                if let Ok(value) = text.parse::<u64>() {
                    return Some(value);
                }
            }
        }
        None
    }

    /// Период protocol-level keep-alive в секундах, обрезается в [5..300].
    ///
    /// Промышленный формат — `keepAliveParams: {keepAliveTimeSeconds: "20",
    /// keepAliveTimeoutSeconds: "20"}`; также принимаем shorter имена
    /// (intervalSeconds / intervalMillis), которые видел TS-reference.
    fn keepalive_secs(&self) -> u64 {
        let Some(value) = &self.keep_alive_params else {
            return DEFAULT_KEEPALIVE_SECS;
        };
        if value.is_number() {
            return value
                .as_u64()
                .unwrap_or(DEFAULT_KEEPALIVE_SECS)
                .clamp(MIN_KEEPALIVE_SECS, MAX_KEEPALIVE_SECS);
        }
        value
            .as_object()
            .and_then(|obj| {
                Self::param_u64(obj, &["keepAliveTimeSeconds", "intervalSeconds"]).or_else(|| {
                    Self::param_u64(obj, &["keepAliveTimeoutSeconds"]).or_else(|| {
                        Self::param_u64(obj, &["intervalMillis"])
                            .map(|m| m / 1000)
                    })
                })
            })
            .unwrap_or(DEFAULT_KEEPALIVE_SECS)
            .clamp(MIN_KEEPALIVE_SECS, MAX_KEEPALIVE_SECS)
    }
}

/// Сформировать handshake: только Origin и Authorization (подтверждено
/// исследованием; User-Agent и X-заголовки для WS не нужны).
fn build_request(
    url: &str,
    token: &str,
    subprotocols: &[&str],
) -> AppResult<tokio_tungstenite::tungstenite::handshake::client::Request> {
    let mut request = IntoClientRequest::into_client_request(url).map_err(|err| {
        tracing::warn!("ws URL invalid: {err}");
        AppError::new(ErrorCode::YandexUnavailable)
    })?;
    request
        .headers_mut()
        .insert("Origin", HeaderValue::from_static("https://music.yandex.ru"));
    let auth_value = format!("OAuth {}", token.trim());
    request.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&auth_value)
            .map_err(|_| AppError::with_details(ErrorCode::InvalidToken, "токен невалидного формата"))?,
    );

    let protocols = subprotocols.join(", ");
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_str(&protocols)
            .map_err(|err| AppError::with_details(ErrorCode::YnisonProtocolChanged, err.to_string()))?,
    );
    Ok(request)
}

/// Подключение к redirect-сервису; читаем один кадр сведений.
async fn fetch_redirect(token: &str) -> AppResult<RedirectInfo> {
    let redirect_subprotocol = device_data_subprotocol(None);
    let target = build_request(
        REDIRECT_URL,
        token,
        &["Bearer", "v2", redirect_subprotocol.as_str()],
    )?;
    let (mut socket, _) = tokio::time::timeout(HANDSHAKE_TIMEOUT, connect_async(target))
        .await
        .map_err(|_| AppError::with_details(ErrorCode::YandexUnavailable, "redirect timeout"))?
        .map_err(map_connect_error)?;

    let raw = tokio::time::timeout(HANDSHAKE_TIMEOUT, socket.next())
        .await
        .map_err(|_| AppError::with_details(ErrorCode::YandexUnavailable, "redirect frame timeout"))?
        .ok_or_else(|| AppError::with_details(ErrorCode::YandexUnavailable, "redirect closed"))?
        .map_err(|err| {
            AppError::with_details(ErrorCode::YandexUnavailable, format!("redirect read: {err}"))
        })?;
    let _ = socket.close(None).await;

    let info: RedirectInfo = match raw {
        Message::Text(text) => {
            let json: serde_json::Value = serde_json::from_str(&text).map_err(bad_redirect_frame)?;
            RedirectInfo::parse(&json)?
        }
        Message::Binary(bytes) => {
            let json: serde_json::Value =
                serde_json::from_slice(bytes.as_ref()).map_err(bad_redirect_frame)?;
            RedirectInfo::parse(&json)?
        }
        _ => {
            return Err(AppError::with_details(
                ErrorCode::YnisonProtocolChanged,
                "unexpected redirect frame".to_string(),
            ))
        }
    };
    if info
        .host
        .as_ref()
        .map(|h| h.is_empty())
        .unwrap_or(true)
    {
        return Err(AppError::with_details(
            ErrorCode::YnisonProtocolChanged,
            "redirect host missing".to_string(),
        ));
    }
    if info.redirect_ticket.is_none() {
        // Не угадываем: сервер обязательно требует тикет на state-сокете.
        return Err(AppError::with_details(
            ErrorCode::YnisonProtocolChanged,
            "redirect ticket missing".to_string(),
        ));
    }
    Ok(info)
}

fn bad_redirect_frame(err: serde_json::Error) -> AppError {
    AppError::with_details(
        ErrorCode::YnisonProtocolChanged,
        format!("redirect frame: {err}"),
    )
}

/// Канонические reference-клиенты используют deviceId короткой hex-строки
/// (SDK — 12 символов, TS — 16). Долгие id (32+) может отклонять сервер
/// (INVALID_ARGUMENT). Берём первые 12 символов стабильного id —
/// по-прежнему уникальны на установку и постоянны между запусками.
fn device_id() -> String {
    use std::sync::OnceLock;
    static DEVICE_ID: OnceLock<String> = OnceLock::new();
    DEVICE_ID
        .get_or_init(|| load_or_create_device_id()[..12].to_string())
        .clone()
}

/// percent-encoded JSON blob устройства для Sec-WebSocket-Protocol.
/// WEB-идентичность (как в TS-reference по умолчанию и Python SDK):
/// app_name "yandex-music", type "1". Должна быть согласована с
/// register_message() (device.info.type = "WEB").
pub fn device_data_subprotocol(redirect: Option<&RedirectInfo>) -> String {
    let info = serde_json::json!({
        "app_name": "yandex-music",
        "type": "1"
    });
    let mut data = serde_json::Map::new();
    data.insert(
        "Ynison-Device-Id".into(),
        serde_json::Value::String(device_id()),
    );
    data.insert(
        "Ynison-Device-Info".into(),
        serde_json::Value::String(info.to_string()),
    );
    if let Some(redirect) = redirect {
        if let Some(ticket) = &redirect.redirect_ticket {
            data.insert(
                "Ynison-Redirect-Ticket".into(),
                serde_json::Value::String(ticket.clone()),
            );
        }
        if let Some(session) = &redirect.session_id {
            data.insert(
                "Ynison-Session-Id".into(),
                serde_json::Value::String(session.clone()),
            );
        }
    }
    let blob = serde_json::Value::Object(data).to_string();
    percent_encode_token(&blob)
}

/// Кодирование в допустимые для `Sec-WebSocket-Protocol` символы (RFC 6455
/// токен; JSON':{'\"' и т.п. проценты-энкодятся).
fn percent_encode_token(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        let c = char::from(*byte);
        let safe = c.is_ascii_alphanumeric()
            || matches!(
                c,
                '-' | '_' | '.' | '~' | '(' | ')' | '*' | '%' | '$' | '!' | '\'' | '+' | '|'
            );
        if safe {
            out.push(c);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Стартовый кадр `updateFullState`: регистрация remote-controller с пустой
/// очередью. Сериализация — строго как в каноническом SDK
/// (`MarshalX/yandex-music-api`): **snake_case ключи + int64 числом** — это
/// `json.dumps(request.to_dict())` в `_base.py._dump_request`. CamelCase
/// и int64-как-строка сервер отклоняет с INVALID_ARGUMENT (400030003).
fn register_message() -> String {
    let device_id = device_id();
    serde_json::json!({
        "update_full_state": {
            "player_state": {
                "status": {
                    "paused": true,
                    "playback_speed": 1.0,
                    "version": {"device_id": device_id, "timestamp_ms": 0},
                },
                "player_queue": {
                    "entity_id": "",
                    "entity_type": "VARIOUS",
                    "entity_context": "BASED_ON_ENTITY_BY_DEFAULT",
                    "from_optional": "",
                    "current_playable_index": -1,
                    "options": {"repeat_mode": "NONE"},
                    "version": {"device_id": device_id},
                }
            },
            "device": {
                "info": {
                    "device_id": device_id,
                    "title": "Yandex Music Presence",
                    "type": "WEB",
                    "app_name": "yandex-music",
                },
                "capabilities": {
                    "can_be_player": false,
                    "can_be_remote_controller": true,
                    "volume_granularity": 0,
                },
                "volume_info": {"volume": 0.0},
            },
            "is_currently_active": false,
        },
        "rid": uuid::Uuid::new_v4().to_string(),
        "player_action_timestamp_ms": 0,
        "activity_interception_type": "DO_NOT_INTERCEPT_BY_DEFAULT",
    })
    .to_string()
}

/// Исход одной сессии взаимодействия.
enum SessionResult {
    Stopped,
    AuthRequired,
    NetworkError,
    ProtocolError,
}

/// События Ynison для координатора.
use crate::sources::{SourceEvent, SourceKind};

struct ClientContext {
    token: String,
    shutdown: CancellationToken,
    shared: Arc<AppSharedState>,
    app_handle: tauri::AppHandle,
    /// Канал Supervisor-арбитра: без него Discord никогда не узнаёт о треке
    /// (UI обновляется через shared, а presence — только через события).
    events: mpsc::UnboundedSender<SourceEvent>,
}

/// Запустить клиент Ynison как фоновую задачу. Токен не логируется.
/// События отдаются в общий контракт `SourceEvent` (см. `sources/mod.rs`).
pub fn spawn_ynison_client(
    token: String,
    shared: Arc<AppSharedState>,
    app_handle: tauri::AppHandle,
    shutdown: CancellationToken,
    events: mpsc::UnboundedSender<SourceEvent>,
) {
    tauri::async_runtime::spawn(async move {
        let ctx = ClientContext {
            token,
            shutdown,
            shared,
            app_handle,
            events,
        };
        let metadata = MetadataClient::new(ctx.token.clone());
        let mut backoff = Backoff::new();

        loop {
            if ctx.shutdown.is_cancelled() {
                ctx.shared
                    .set_ynison_status(YnisonStatus::Disconnected, &ctx.app_handle);
                let _ = ctx.events.send(SourceEvent::Stopped { kind: SourceKind::Ynison });
                return;
            }
            ctx.shared
                .set_ynison_status(YnisonStatus::Connecting, &ctx.app_handle);

            match run_session(&ctx, &metadata, &mut backoff).await {
                SessionResult::Stopped => {
                    ctx.shared
                        .set_ynison_status(YnisonStatus::Disconnected, &ctx.app_handle);
                    let _ = ctx.events.send(SourceEvent::Stopped { kind: SourceKind::Ynison });
                    return;
                }
                SessionResult::AuthRequired => {
                    ctx.shared.set_ynison_status(
                        YnisonStatus::AuthenticationRequired,
                        &ctx.app_handle,
                    );
                    let _ = ctx.events.send(SourceEvent::RequiresAuth);
                    let _ = ctx.events.send(SourceEvent::State {
                        kind: SourceKind::Ynison,
                        state: None,
                    });
                    return;
                }
                SessionResult::NetworkError => {
                    tracing::warn!("Ynison connection lost, будет переподключение");
                    // Не оставляем «Подключено» в UI на всё время задержки.
                    ctx.shared
                        .set_ynison_status(YnisonStatus::Reconnecting, &ctx.app_handle);
                }
                SessionResult::ProtocolError => {
                    ctx.shared
                        .set_ynison_status(YnisonStatus::ProtocolError, &ctx.app_handle);
                    tracing::warn!("Ynison protocol error");
                }
            }

            let delay = jitter(backoff.next_delay());
            tracing::warn!("Ynison: переподключение через {} мс", delay.as_millis());
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = ctx.shutdown.cancelled() => {
                    ctx.shared
                        .set_ynison_status(YnisonStatus::Disconnected, &ctx.app_handle);
                    let _ = ctx.events.send(SourceEvent::Stopped { kind: SourceKind::Ynison });
                    return;
                }
            }
        }
    });
}

/// Одна сессия: redirect handshake + подключение к состоянию + цикл кадров.
/// `backoff` передаётся, чтобы сбросить его ТОЛЬКО при успешном подключении
/// (раньше он сбрасывался после каждой неудачи — ретраи били без паузы).
async fn run_session(
    ctx: &ClientContext,
    metadata: &MetadataClient,
    backoff: &mut Backoff,
) -> SessionResult {
    // Шаг 1: redirect.
    let redirect = match fetch_redirect(&ctx.token).await {
        Ok(info) => info,
        Err(err) => return classify_session_error(err),
    };
    // (host и формат тикета не секреты; сам тикет — ок, не логируем целиком)
    tracing::info!(
        "Ynison redirect: host={:?} ticket={} session_id={:?}",
        redirect.host.as_deref().unwrap_or("<none>"),
        match redirect.redirect_ticket.as_deref() {
            Some(t) => format!("len={}, prefix={:?}", t.len(), char_prefix(t, 8)),
            None => "<none>".to_string(),
        },
        redirect.session_id.as_deref().map(|s| char_prefix(s, 6)),
    );
    let keepalive_secs = redirect.keepalive_secs();

    // Шаг 2: state stream.
    let Some(host) = redirect.host.clone().filter(|h| !h.is_empty()) else {
        return SessionResult::ProtocolError;
    };
    let url = format!("wss://{host}/ynison_state.YnisonStateService/PutYnisonState");
    let state_sub = device_data_subprotocol(Some(&redirect));
    let request = match build_request(
        &url,
        &ctx.token,
        &["Bearer", "v2", state_sub.as_str()],
    ) {
        Ok(req) => req,
        Err(err) => return classify_session_error(err),
    };

    let connect_future = connect_async(request);
    let (mut socket, _response) = match tokio::time::timeout(HANDSHAKE_TIMEOUT, connect_future).await
    {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => return classify_session_error(map_connect_error(err)),
        Err(_) => {
            tracing::warn!("ws handshake failed: state timeout");
            return classify_session_error(AppError::with_details(
                ErrorCode::YandexUnavailable,
                "state handshake timeout",
            ));
        }
    };

    // Регистрация устройства.
    if let Err(err) = socket.send(Message::text(register_message())).await {
        tracing::warn!("register frame не отправлен: {err}");
        return SessionResult::NetworkError;
    }
    tracing::info!("Ynison: register отправлен");
    // ВАЖНО: backoff НЕ сбрасываем здесь. Рукопожатие и register — не
    // доказательство живой сессии (ошибка приходит позже, в message_loop):
    // если сбрасывать сейчас, попытки вечно идут с базовой задержкой и
    // штормят Ynison (сервер начинает отвечать UNAVAILABLE).
    ctx.shared
        .set_ynison_status(YnisonStatus::Connected, &ctx.app_handle);

    message_loop(ctx, metadata, socket, keepalive_secs, backoff).await
}

/// Цикл чтения кадров + исходящие protocol-level pings.
async fn message_loop(
    ctx: &ClientContext,
    metadata: &MetadataClient,
    mut socket: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    keepalive_secs: u64,
    backoff: &mut Backoff,
) -> SessionResult {
    let started = std::time::Instant::now();
    let mut ping_interval =
        tokio::time::interval(Duration::from_secs(keepalive_secs.max(1)));
    // Первый tick у interval срабатывает моментально — пропускаем его.
    ping_interval.tick().await;
    let mut first_frame_logged = false;
    // Backoff сбрасывается только после того, как сессия продержалась
    // >=15 c без разрыва: receive-first-frame / connect — недостаточный
    // критерий (сервер шлёт снапшот и сразу может отдать error/503).
    const BACKOFF_RESET_AFTER: Duration = Duration::from_secs(15);
    let mut backoff_reset = false;

    loop {
        if !backoff_reset && started.elapsed() >= BACKOFF_RESET_AFTER {
            backoff.reset();
            backoff_reset = true;
            tracing::info!("Ynison: сессия стабильна ({} c), backoff сброшен", BACKOFF_RESET_AFTER.as_secs());
        }
        tokio::select! {
            _ = ctx.shutdown.cancelled() => {
                let _ = socket.close(None).await;
                return SessionResult::Stopped;
            }

            _ = ping_interval.tick() => {
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    tracing::warn!("ping не отправлен");
                    return SessionResult::NetworkError;
                }
            }

            message = socket.next() => {
                match message {
                    // EOF без close-кадра: обычно сервер разорвал TCP (таймаут
                    // keep-alive или отказ от сессии) — обязаны видеть в логе.
                    None => {
                        tracing::warn!(
                            "ws EOF без close-кадра: сессия прожила {} мс",
                            started.elapsed().as_millis()
                        );
                        return SessionResult::NetworkError;
                    }
                    Some(Err(err)) => {
                        tracing::warn!(
                            "ws frame от сервера: {} (сессия {} мс)",
                            ws_error_class(&err),
                            started.elapsed().as_millis()
                        );
                        return SessionResult::NetworkError;
                    }
                    // Отвечаем pong сразу: без этого сервер считает клиент
                    // мёртвым и рвёт соединение (реальная причина «connection lost»).
                    Some(Ok(Message::Ping(payload))) => {
                        if socket.send(Message::Pong(payload)).await.is_err() {
                            tracing::warn!("pong не отправлен");
                            return SessionResult::NetworkError;
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        if !first_frame_logged {
                            first_frame_logged = true;
                            tracing::info!(
                                "Ynison: получили первый кадр состояния ({} мс)",
                                started.elapsed().as_millis()
                            );
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(text.as_str()) {
                                tracing::info!("Ynison первый кадр: {}", safe_frame_summary(&json));
                            }
                        }
                        if text.len() > MAX_FRAME_BYTES {
                            tracing::warn!(
                                "text frame превышает лимит {} байт — кадр пропущен",
                                MAX_FRAME_BYTES
                            );
                            continue;
                        }
                        match dispatch_frame(&text, metadata).await {
                            Ok(state) => deliver_state(ctx, state),
                            Err(err) => return classify_session_error(err),
                        }
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        if !first_frame_logged {
                            first_frame_logged = true;
                            tracing::info!(
                                "Ynison: получили первый кадр состояния (binary, {} байт, {} мс)",
                                bytes.len(),
                                started.elapsed().as_millis()
                            );
                        }
                        // Тот же лимит размера, что и для текстовых кадров.
                        if bytes.len() > MAX_FRAME_BYTES {
                            tracing::warn!(
                                "binary frame превышает лимит {} байт — кадр пропущен",
                                MAX_FRAME_BYTES
                            );
                            continue;
                        }
                        match std::str::from_utf8(bytes.as_ref()) {
                            Ok(text) => {
                                // Обработка ошибок как в text-ветке: error-кадр
                                // может прийти binary и должен уметь уводить
                                // сессию в AuthRequired/ProtocolError.
                                match dispatch_frame(text, metadata).await {
                                    Ok(state) => deliver_state(ctx, state),
                                    Err(err) => return classify_session_error(err),
                                }
                            }
                            Err(_) => {
                                tracing::warn!("binary frame не utf8");
                            }
                        }
                    }
                    Some(Ok(Message::Close(frame))) => {
                        // Код close-кадра — ключевая диагностика (1008 policy,
                        // 4xxx авторизация и т.п.). Секретов не содержит.
                        let detail = match &frame {
                            Some(close) => {
                                format!(
                                    "code={} reason={}",
                                    close.code,
                                    close.reason.to_string()
                                )
                            }
                            None => "без кадра".to_string(),
                        };
                        tracing::warn!(
                            "соединение закрыто сервером ({detail}, сессия {} мс)",
                            started.elapsed().as_millis()
                        );
                        return SessionResult::NetworkError;
                    }
                    Some(Ok(_)) => {}
                }
            }
        }
    }
}

/// Безопасная сводка кадра для диагностики: структура есть/нет, счётчики,
/// заголовок трека (это отображаемые данные, не секреты; полный кадр не логируем).
fn safe_frame_summary(json: &serde_json::Value) -> String {
    let get = |name: &str| json.get(name).or_else(|| json.get(&to_camel(name)));
    let queue = get("player_queue").or_else(|| {
        get("player_state").and_then(|s| s.get("player_queue").or_else(|| s.get("playerQueue")))
    });
    let state = get("player_state");
    let mut parts = Vec::new();
    parts.push(format!(
        "player_state={}",
        state.map(|_| "есть").unwrap_or("нет")
    ));
    match queue {
        None => parts.push("player_queue=нет".to_string()),
        Some(q) => {
            let list = q.get("playable_list").or_else(|| q.get("playableList"));
            let idx = q
                .get("current_playable_index")
                .or_else(|| q.get("currentPlayableIndex"))
                .and_then(|v| match v {
                    serde_json::Value::Number(n) => n.as_i64(),
                    serde_json::Value::String(s) => s.parse().ok(),
                    _ => None,
                });
            let list_len = list.and_then(|l| l.as_array()).map(|a| a.len()).unwrap_or(0);
            let title = list
                .and_then(|l| l.as_array())
                .and_then(|a| {
                    idx.and_then(|i| usize::try_from(i).ok())
                        .and_then(|i| a.get(i))
                })
                .and_then(|t| {
                    t.get("title")
                        .or_else(|| t.get("playable_id"))
                        .or_else(|| t.get("playableId"))
                        .and_then(|v| v.as_str())
                });
            parts.push(format!("player_queue=len={list_len}, index={idx:?}"));
            if let Some(t) = title {
                parts.push(format!("трек={t:?}"));
            }
        }
    }
    if let Some(s) = state {
        if let Some(status) = s.get("status") {
            let paused = status
                .get("paused")
                .and_then(|v| v.as_bool())
                .map(|p| if p { "пауза" } else { "играет" });
            if let Some(p) = paused {
                parts.push(format!("статус={p}"));
            }
        }
    }
    parts.join(", ")
}

/// "player_queue" → "playerQueue" (для tolerant поиска в сводке).
fn to_camel(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut upper = false;
    for ch in name.chars() {
        if ch == '_' {
            upper = true;
        } else if upper {
            out.extend(ch.to_uppercase());
            upper = false;
        } else {
            out.push(ch);
        }
    }
    out
}

/// Доставить состояние Supervisor-арбитру: он решает, публиковать ли
/// его в UI-снапшот и координатору (один хоп, без дублирования).
fn deliver_state(ctx: &ClientContext, state: Option<PlaybackState>) {
    let _ = ctx.events.send(SourceEvent::State {
        kind: SourceKind::Ynison,
        state,
    });
}

/// Разбор одного сообщения. Возвращает нормализованное состояние.
async fn dispatch_frame(
    text: &str,
    metadata: &MetadataClient,
) -> AppResult<Option<PlaybackState>> {
    // ВАЖНО: никогда не логируем текст кадра (личные данные).
    let json: serde_json::Value = serde_json::from_str(text).map_err(|err| {
        AppError::with_details(ErrorCode::YnisonProtocolChanged, format!("frame parse: {err}"))
    })?;

    // Ошибки grpc по спецификации (исследование: JSON кадры ошибки выглядят
    // как {"error": {"grpcCode": …, "httpCode": …, "details": …}}; также
    // встречается плоская форма grpcCode/alert верхнего уровня).
    let error_obj = json.get("error").filter(|e| e.is_object());
    if let Some(err_obj) = error_obj {
        // Код может лежать в camelCase (grpcCode) или snake_case (grpc_code),
        // техдетали — в details либо extra_headers/extraHeaders.
        let grpc = err_obj
            .get("grpcCode")
            .or_else(|| err_obj.get("grpc_code"))
            .and_then(|c| c.as_i64());
        let http = err_obj
            .get("httpCode")
            .or_else(|| err_obj.get("http_code"))
            .and_then(|c| c.as_i64());
        let yms = err_obj
            .get("details")
            .or_else(|| err_obj.get("extra_headers"))
            .or_else(|| err_obj.get("extraHeaders"))
            .and_then(|d| {
                d.get("ynison-error-code")
                    .or_else(|| d.get("ynison_error_code"))
            })
            .and_then(|v| v.as_str())
            .to_owned();
        // Текст ошибки сервера не содержит наших секретов — логируем:
        // он указывает, какой именно аргумент отклонён (grpc 3).
        let message = err_obj
            .get("message")
            .and_then(|m| m.as_str())
            .to_owned();
        tracing::warn!(
            "Ynison error frame: grpcCode={grpc:?} httpCode={http:?} yms={yms:?} message={message:?}"
        );
        match (grpc, http) {
            (Some(16), _) | (Some(7), _) | (_, Some(401)) | (_, Some(403)) => {
                return Err(AppError::new(ErrorCode::InvalidToken));
            }
            // UNAVAILABLE / 503 — upstream временно недоступен: это сетевой
            // класс, рестарт по backoff (не ProtocolError).
            (Some(14), _) | (_, Some(503)) | (_, Some(504)) => {
                return Err(AppError::new(ErrorCode::YandexUnavailable));
            }
            _ => {}
        }
        if yms.as_deref() == Some("400090001") {
            tracing::warn!("Ynison: displacement (два устройства с одним device id)");
        }
        return Err(AppError::with_details(
            ErrorCode::YnisonProtocolChanged,
            format!("error frame grpc={grpc:?} http={http:?} yms={yms:?}"),
        ));
    }
    if let Some(code) = json.get("grpcCode").and_then(|c| c.as_i64()) {
        if code == 400_090_001 {
            tracing::warn!("Ynison: displacement (два устройства с одним device id)");
        } else {
            tracing::warn!("Ynison grpc code {code}");
        }
        return Err(AppError::with_details(
            ErrorCode::YnisonProtocolChanged,
            format!("grpc code {code}"),
        ));
    }
    // auth отклонён — сервер отдаёт authRejection по невалидному токену.
    if json.get("authRejection").is_some() {
        return Err(AppError::new(ErrorCode::InvalidToken));
    }

    let mut frame = parse_frame(&json).ok_or_else(|| {
        AppError::with_details(
            ErrorCode::YnisonProtocolChanged,
            "кадр не соответствует схеме".to_string(),
        )
    })?;

    // Коррекция ложной паузы (см. presence/smtc.rs, референс WYMR):
    // Ynison иногда пишет paused=true при работающем воспроизведении.
    // Сверяемся с реальными SMTC-сессиями Windows: если та же композиция
    // там «играет», считаем паузу сервера ложной и публикуем Playing.
    if is_frame_paused(&frame) {
        if let Some(title) = active_playable_title(&frame) {
            if pause_corrected_by_smtc(&title).await {
                tracing::info!("Ynison: ложная пауза исправлена по SMTC ({title:?})");
                if let Some(status) = frame
                    .player_state
                    .as_mut()
                    .and_then(|ps| ps.status.as_mut())
                {
                    status.paused = Some(false);
                }
            }
        }
    }

    let now = std::time::SystemTime::now();
    let mut state = normalize(&frame, now);
    // Подтягиваем недостающие метаданные по track id (ТЗ §5.4): только при
    // отсутствии полей в live-state.
    if let Some(state) = state.as_mut() {
        if state.artists.is_empty() {
            if let Some(id) = state.track_id.clone() {
                match metadata.lookup(&id).await {
                    Ok(Some(meta)) => {
                        if state.title.is_none() {
                            state.title = meta.title;
                        }
                        state.artists = meta.artists;
                        if state.album.is_none() {
                            state.album = meta.album;
                        }
                        if state.cover_url.is_none() {
                            state.cover_url = meta.cover_url;
                        }
                        if state.duration_ms.is_none() {
                            state.duration_ms = meta.duration_ms;
                        }
                    }
                    Ok(None) | Err(_) => {
                        // Продолжаем с полями live-state (ТЗ §5.4).
                    }
                }
            }
        }
    }

    Ok(state)
}

/// Статус кадра — «пауза» (используется перед SMTC-коррекцией).
fn is_frame_paused(frame: &crate::yandex::protocol::YnisonFrame) -> bool {
    frame
        .player_state
        .as_ref()
        .and_then(|ps| ps.status.as_ref())
        .and_then(|s| s.paused)
        == Some(true)
}

/// Заголовок АКТИВНОГО playable — по `current_playable_index` (как в
/// нормализации), а не `first()`: иначе SMTC-сверка может сравнивать не тот
/// трек, когда очередь сдвинулась.
fn active_playable_title(frame: &crate::yandex::protocol::YnisonFrame) -> Option<String> {
    let queue = frame
        .player_queue
        .as_ref()
        .or_else(|| frame.player_state.as_ref().and_then(|s| s.player_queue.as_ref()))?;
    let list = queue.playable_list.as_ref()?;
    let playable = match queue.current_playable_index.as_ref().and_then(|v| v.as_i64()) {
        Some(index) if index >= 0 => list.get(usize::try_from(index).ok()?)?,
        Some(_) => return None, // отрицательный индекс = очередь пуста
        None => list.first()?,
    };
    playable
        .title
        .as_deref()
        .map(str::to_string)
        .filter(|t| !t.trim().is_empty())
}

/// SMTC-сверка в выделенном потоке с таймаутом: COM-вызовы блокируют, а
/// WS-цикл не должен подвисать (коррекция в худшем случае не применяется).
async fn pause_corrected_by_smtc(title: &str) -> bool {
    let title = title.to_string();
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::task::spawn_blocking(move || {
            crate::sources::smtc::helpers::smtc_reports_playing(&title)
        }),
    )
    .await;
    match outcome {
        Ok(Ok(says_playing)) => says_playing,
        Ok(Err(join)) => {
            tracing::debug!("SMTC task join failed: {join:?}");
            false
        }
        Err(_) => {
            tracing::debug!("SMTC check timed out (2 c)");
            false
        }
    }
}

/// Сопоставить ошибку с элементом SessionResult.
fn classify_session_error(err: AppError) -> SessionResult {
    match err.code {
        ErrorCode::InvalidToken | ErrorCode::TokenExpired | ErrorCode::NoToken => {
            SessionResult::AuthRequired
        }
        ErrorCode::YnisonProtocolChanged => SessionResult::ProtocolError,
        _ => SessionResult::NetworkError,
    }
}

/// Ренейм ошибок не требуем: без паник на границе байт/символ
/// (байтовый слайс `&s[..6]` упал бы на не-ASCII).
fn char_prefix(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

/// Классификация ws-ошибок безопасным текстом без секретов.
fn ws_error_class(err: &tokio_tungstenite::tungstenite::Error) -> String {
    use tokio_tungstenite::tungstenite::Error as WsError;
    match err {
        WsError::Http(response) => format!("http {}", response.status()),
        WsError::HttpFormat(_) => "handshake format".to_string(),
        WsError::Io(err) => format!("io: {err}"),
        WsError::Tls(err) => format!("tls: {err}"),
        WsError::Url(err) => format!("url: {err}"),
        WsError::Protocol(_) => "ws protocol".to_string(),
        WsError::Capacity(_) => "capacity".to_string(),
        WsError::ConnectionClosed => "connection closed".to_string(),
        WsError::AlreadyClosed => "already closed".to_string(),
        WsError::WriteBufferFull(_) => "write buffer full".to_string(),
        WsError::Utf8(_) => "utf8".to_string(),
        WsError::AttackAttempt => "attack attempt".to_string(),
    }
}

/// Ошибка соединения → AppError: HTTP 401/403 означает «токен отклонён»
/// (UI покажет карточку авторизации вместо вечного reconnect), остальное
/// — недоступность сервиса. Причина всегда логируется.
fn map_connect_error(err: tokio_tungstenite::tungstenite::Error) -> AppError {
    use tokio_tungstenite::tungstenite::Error as WsError;
    let reason = ws_error_class(&err);
    tracing::warn!("ws handshake failed: {reason}");
    if let WsError::Http(response) = &err {
        let status = response.status().as_u16();
        if status == 401 || status == 403 {
            return AppError::with_details(
                ErrorCode::InvalidToken,
                format!("сервер отклонил токен (HTTP {status})"),
            );
        }
    }
    AppError::with_details(ErrorCode::YandexUnavailable, reason)
}

/// Jitter ±20% к задержке (rng — простой xorshift от времени).
fn jitter(base: Duration) -> Duration {
    let millis = base.as_millis() as u64;
    let window = (millis / 5).max(1);
    let noise = crate::rand_between(window * 2 + 1);
    Duration::from_millis(
        millis
            .saturating_sub(window)
            .saturating_add(noise.min(window * 2)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keepalive_defaults_to_20() {
        let info = RedirectInfo::default();
        assert_eq!(info.keepalive_secs(), 20);
    }

    #[test]
    fn keepalive_respects_interval_seconds() {
        let info = RedirectInfo {
            keep_alive_params: Some(serde_json::json!({"intervalSeconds": 12})),
            ..RedirectInfo::default()
        };
        assert_eq!(info.keepalive_secs(), 12);
    }

    #[test]
    fn keepalive_respects_interval_millis() {
        let info = RedirectInfo {
            keep_alive_params: Some(serde_json::json!({"intervalMillis": 25_000})),
            ..RedirectInfo::default()
        };
        assert_eq!(info.keepalive_secs(), 25);
    }

    #[test]
    fn keepalive_clamps_out_of_range() {
        let info = RedirectInfo {
            keep_alive_params: Some(serde_json::json!(600)),
            ..RedirectInfo::default()
        };
        assert_eq!(info.keepalive_secs(), 300);
    }

    #[test]
    fn device_subprotocol_is_token_safe() {
        let blob = device_data_subprotocol(None);
        assert!(blob.starts_with("%7B"), "открывается percent-encoded");
        for c in blob.chars() {
            assert!(
                c.is_ascii_alphanumeric()
                    || "-_~.()*%$!'|+".contains(c),
                "недопустимый символ в subprotocol: {c}"
            );
        }
        // Декодируемся обратно в валидный JSON.
        let bytes = percent_decode_for_test(&blob);
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("должен декодироваться");
        assert!(json.get("Ynison-Device-Id").is_some());
        assert!(json["Ynison-Device-Info"].is_string());
    }

    #[test]
    fn device_id_is_stable_hex12() {
        let id = device_id();
        assert_eq!(id.len(), 12, "канонические reference-клиенты шлют 12–16 hex");
        assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(id, device_id(), "device id стабилен");
    }

    #[test]
    fn register_message_is_valid_json_with_rid() {
        let msg = register_message();
        let json: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert!(json.get("rid").is_some());
        // Канонический wire-формат SDK: snake_case + числовые int64.
        let full = &json["update_full_state"];
        assert_eq!(full["device"]["info"]["type"], "WEB");
        assert_eq!(full["device"]["info"]["app_name"], "yandex-music");
        assert_eq!(full["device"]["capabilities"]["can_be_player"], false);
        assert_eq!(full["device"]["capabilities"]["can_be_remote_controller"], true);
        assert_eq!(full["device"]["volume_info"]["volume"], 0.0);
        assert_eq!(full["player_state"]["status"]["paused"], true);
        assert_eq!(full["player_state"]["status"]["playback_speed"], 1.0);
        assert_eq!(full["player_state"]["status"]["version"]["timestamp_ms"], 0);
        let queue = &full["player_state"]["player_queue"];
        assert_eq!(queue["entity_type"], "VARIOUS");
        assert_eq!(queue["entity_context"], "BASED_ON_ENTITY_BY_DEFAULT");
        assert_eq!(queue["current_playable_index"], -1);
        assert_eq!(queue["options"]["repeat_mode"], "NONE");
        assert_eq!(json["player_action_timestamp_ms"], 0);
        assert_eq!(json["activity_interception_type"], "DO_NOT_INTERCEPT_BY_DEFAULT");
    }

    fn percent_decode_for_test(input: &str) -> Vec<u8> {
        let bytes = input.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() + 1 && i + 2 <= bytes.len() - 1 {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap();
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
            }
            out.push(bytes[i]);
            i += 1;
        }
        out
    }
}
