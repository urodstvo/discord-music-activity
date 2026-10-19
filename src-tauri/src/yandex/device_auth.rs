//! OAuth Device Flow для входа через Яндекс (RFC 8628, Yandex-реализация).
//!
//! Зачем: ручной ввод OAuth-токена (ТЗ §5.1) неудобен — токен приходится
//! доставать из cookie/devtools. Device Flow даёт штатный UX: приложение
//! показывает короткий `user_code`, пользователь вводит его на странице
//! `oauth.yandex.ru/device` и подтверждает доступ, приложение опрашивает
//! `POST /token` и получает `access_token` автоматически.
//!
//! Используются публичные OAuth-креды официального Android-приложения
//! Яндекс Музыки — те же, что встроены в открытые клиенты (напр.
//! `MarshalX/yandex-music-api`, `sonrac/yandex-music-api`). Это не секреты
//! проекта: они извлекаемы из публичного APK и одинаковы у всех
//! пользователей. Собственная регистрация OAuth-приложения потребовала бы
//! от каждого пользователя заводить Client ID/Secret — это хуже по UX.
//!
//! Секреты: `access_token` никогда не логируется и не возвращается наружу;
//! сохраняется только в Credential Manager через `yandex::auth`.

use std::time::Duration;

use crate::yandex::errors::{AppError, AppResult, ErrorCode};

/// Публичный Client ID Android-приложения Яндекс Музыки.
pub const DEVICE_CLIENT_ID: &str = "23cabbbdc6cd418abb4b39c32c41195d";
/// Публичный Client Secret того же приложения (см. комментарий модуля).
pub const DEVICE_CLIENT_SECRET: &str = "53bc75238f0c4d08a118e51fe9203300";
/// Базовый URL OAuth-сервера Яндекса.
pub const OAUTH_BASE_URL: &str = "https://oauth.yandex.ru";
/// Имя устройства, видимое пользователю на странице подтверждения.
pub const DEVICE_NAME: &str = "Яндекс Музыка → Discord (Windows)";

/// Информация для пользователя: что ввести и где.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCodeInfo {
    pub user_code: String,
    pub verification_url: String,
    /// Полный интервал опроса, рекомендованный сервером (сек).
    pub interval_secs: u64,
    /// Время жизни пары кодов (сек).
    pub expires_in_secs: u64,
}

/// Сырой ответ `POST /device/code` (толерантен к `verification_uri`).
#[derive(Debug, serde::Deserialize)]
struct DeviceCodeResponse {
    #[serde(default)]
    device_code: Option<String>,
    #[serde(default)]
    user_code: Option<String>,
    #[serde(default)]
    verification_url: Option<String>,
    #[serde(default)]
    verification_uri: Option<String>,
    #[serde(default)]
    interval: Option<u64>,
    #[serde(default)]
    expires_in: Option<u64>,
}

/// Сырой ответ `POST /token`.
#[derive(Debug, serde::Deserialize)]
struct TokenSuccess {
    #[serde(default)]
    access_token: Option<String>,
}

/// Ошибка OAuth от `/token` (`authorization_pending` и др.).
#[derive(Debug, serde::Deserialize)]
struct OAuthErrorBody {
    #[serde(default)]
    error: Option<String>,
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Запросить пару кодов (`device_code` + `user_code`).
pub async fn request_device_code() -> AppResult<(DeviceCodeInfo, String)> {
    let device_id = crate::config::persistence::load_or_create_device_id();
    let client = http_client();
    let url = format!("{OAUTH_BASE_URL}/device/code");
    let response = client
        .post(&url)
        .form(&[
            ("client_id", DEVICE_CLIENT_ID),
            ("device_id", device_id.as_str()),
            ("device_name", DEVICE_NAME),
        ])
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|_| AppError::new(ErrorCode::YandexUnavailable))?;

    if !response.status().is_success() {
        tracing::warn!("device/code: HTTP {}", response.status());
        return Err(map_device_code_status(response.status().as_u16()));
    }
    let raw: DeviceCodeResponse = response.json().await.map_err(|_| {
        AppError::with_details(ErrorCode::YandexUnavailable, "device/code bad json")
    })?;
    parse_device_code_response(raw)
}

/// Разобрать ответ `/device/code` в пару (инфо для UI, код для опроса).
fn parse_device_code_response(raw: DeviceCodeResponse) -> AppResult<(DeviceCodeInfo, String)> {
    let device_code = raw
        .device_code
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            AppError::with_details(ErrorCode::YandexUnavailable, "device/code без device_code")
        })?;
    let user_code = raw
        .user_code
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            AppError::with_details(ErrorCode::YandexUnavailable, "device/code без user_code")
        })?;
    let verification_url = raw
        .verification_url
        .or(raw.verification_uri)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "https://oauth.yandex.ru/device".to_string());
    let info = DeviceCodeInfo {
        user_code,
        verification_url,
        interval_secs: raw.interval.unwrap_or(5).clamp(2, 10),
        expires_in_secs: raw.expires_in.unwrap_or(300).clamp(60, 1800),
    };
    Ok((info, device_code))
}

/// Исход одного опроса `/token`.
#[derive(Debug, PartialEq, Eq)]
pub enum PollOutcome {
    /// Пользователь ещё не подтвердил — продолжать опрос.
    Pending,
    /// Получен токен (уже проверен на непустоту).
    Authorized(String),
}

/// Однократный опрос статуса: `Ok(Pending)` — ждать дальше.
pub async fn poll_device_token_once(device_code: &str) -> AppResult<PollOutcome> {
    let client = http_client();
    let url = format!("{OAUTH_BASE_URL}/token");
    let response = client
        .post(&url)
        .form(&[
            ("grant_type", "device_code"),
            ("code", device_code),
            ("client_id", DEVICE_CLIENT_ID),
            ("client_secret", DEVICE_CLIENT_SECRET),
        ])
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|_| AppError::new(ErrorCode::YandexUnavailable))?;

    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    parse_token_poll_body(status, &body)
}

/// Чистая функция разбора ответа опроса — покрыта unit-тестами без сети.
fn parse_token_poll_body(status: u16, body: &str) -> AppResult<PollOutcome> {
    if status == 200 {
        let ok: TokenSuccess = serde_json::from_str(body)
            .map_err(|_| AppError::with_details(ErrorCode::YandexUnavailable, "token bad json"))?;
        let token = ok
            .access_token
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                AppError::with_details(ErrorCode::YandexUnavailable, "token без access_token")
            })?;
        return Ok(PollOutcome::Authorized(token));
    }
    // Ошибка OAuth: смотрим поле `error`.
    let err: OAuthErrorBody = serde_json::from_str(body).unwrap_or(OAuthErrorBody { error: None });
    match err.error.as_deref() {
        Some("authorization_pending") => Ok(PollOutcome::Pending),
        // Сервер просит снизить темп — трактуем как pending, интервал
        // увеличивает вызывающий цикл.
        Some("slow_down") => Ok(PollOutcome::Pending),
        Some("access_denied") => Err(AppError::new(ErrorCode::DeviceAuthDenied)),
        Some("expired_token") | Some("invalid_grant") => {
            Err(AppError::new(ErrorCode::DeviceAuthExpired))
        }
        Some("invalid_client") | Some("unauthorized_client") => Err(AppError::with_details(
            ErrorCode::YandexUnavailable,
            "oauth client отклонён",
        )),
        _ => Err(AppError::with_details(
            ErrorCode::YandexUnavailable,
            format!("token poll HTTP {status}"),
        )),
    }
}

fn map_device_code_status(status: u16) -> AppError {
    match status {
        400..=499 => AppError::with_details(ErrorCode::YandexUnavailable, "device/code отклонён"),
        _ => AppError::new(ErrorCode::YandexUnavailable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_device_code_response() {
        let raw: DeviceCodeResponse = serde_json::from_str(
            r#"{"device_code":"abc","user_code":"h5nbcr6c","verification_url":"https://oauth.yandex.ru/device","interval":5,"expires_in":300}"#,
        )
        .unwrap();
        let (info, code) = parse_device_code_response(raw).unwrap();
        assert_eq!(code, "abc");
        assert_eq!(info.user_code, "h5nbcr6c");
        assert_eq!(info.interval_secs, 5);
        assert_eq!(info.expires_in_secs, 300);
    }

    #[test]
    fn accepts_verification_uri_alias() {
        let raw: DeviceCodeResponse = serde_json::from_str(
            r#"{"device_code":"abc","user_code":"x","verification_uri":"https://oauth.yandex.com/device"}"#,
        )
        .unwrap();
        let (info, _) = parse_device_code_response(raw).unwrap();
        assert!(info.verification_url.contains("device"));
        // Дефолты при отсутствии interval/expires_in.
        assert_eq!(info.interval_secs, 5);
        assert_eq!(info.expires_in_secs, 300);
    }

    #[test]
    fn rejects_device_code_without_codes() {
        let raw: DeviceCodeResponse = serde_json::from_str(r#"{"interval":5}"#).unwrap();
        assert!(parse_device_code_response(raw).is_err());
    }

    #[test]
    fn poll_pending_and_slow_down() {
        let body = r#"{"error":"authorization_pending"}"#;
        assert_eq!(
            parse_token_poll_body(400, body).unwrap(),
            PollOutcome::Pending
        );
        let slow = r#"{"error":"slow_down"}"#;
        assert_eq!(
            parse_token_poll_body(400, slow).unwrap(),
            PollOutcome::Pending
        );
    }

    #[test]
    fn poll_success_returns_token() {
        let body = r#"{"access_token":"tok123","token_type":"bearer","expires_in":999}"#;
        assert_eq!(
            parse_token_poll_body(200, body).unwrap(),
            PollOutcome::Authorized("tok123".to_string())
        );
    }

    #[test]
    fn poll_denied_and_expired_map_to_codes() {
        let denied = parse_token_poll_body(400, r#"{"error":"access_denied"}"#).unwrap_err();
        assert_eq!(denied.code, ErrorCode::DeviceAuthDenied);
        let expired = parse_token_poll_body(400, r#"{"error":"expired_token"}"#).unwrap_err();
        assert_eq!(expired.code, ErrorCode::DeviceAuthExpired);
        let grant = parse_token_poll_body(400, r#"{"error":"invalid_grant"}"#).unwrap_err();
        assert_eq!(grant.code, ErrorCode::DeviceAuthExpired);
    }
}
