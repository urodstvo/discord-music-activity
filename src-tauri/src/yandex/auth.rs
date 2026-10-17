//! Авторизация: проверка/чтение/хранение/удаление OAuth-токена (ТЗ §5.1).
//!
//! Токен живёт только в Windows Credential Manager (через crate `keyring`).
//! Запрещено сохранять его в конфиге, логах, frontend storage или аргументах
//! процессов.

use keyring::Entry;

use crate::logging::SensitiveGuard;
use crate::yandex::errors::{AppError, AppResult, ErrorCode};

/// Имя сервиса в credential store. Использовать зафиксированное значение,
/// чтобы при переустановке приложения записи были обнаружены.
pub const CREDENTIAL_SERVICE: &str = "yandex-music-presence";
/// Имя записи.
pub const CREDENTIAL_USER: &str = "yandex-music-oauth-token";

/// Проверить токен невязким запросом к публичному endpoint каталога
/// Яндекс Музыки. Токен не возвращается из этой функции наружу.
///
/// Таймаут сетевого вызова обязательно задаётся (ТЗ §4.3).
pub async fn validate_token(token: &str) -> AppResult<()> {
    if token.trim().is_empty() {
        return Err(AppError::new(ErrorCode::NoToken));
    }

    // Проверка — короткий GET к каталогу Authorization-заголовком.
    let client = reqwest_client();
    let url = url::Url::parse("https://api.music.yandex.ru/account/status")
        .expect("hardcoded https url must be valid");
    let response = client
        .get(url)
        .header("Authorization", format!("OAuth {}", token.trim()))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|_err| {
            tracing::warn!("validate_token: сеть недоступна");
            AppError::new(ErrorCode::YandexUnavailable)
        })?;

    match response.status().as_u16() {
        200 => Ok(()),
        401 | 403 => Err(AppError::new(ErrorCode::InvalidToken)),
        code if code >= 500 => Err(AppError::new(ErrorCode::YandexUnavailable)),
        code => Err(AppError::with_details(
            ErrorCode::InvalidToken,
            format!("server status {}", code),
        )),
    }
}

fn reqwest_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Сохранить токен в системное хранилище. Регистрируем секрет как «sensitive»,
/// чтобы любая случайная попытка вывода была замаскирована.
pub fn save_token(token: &str) -> AppResult<()> {
    let entry = Entry::new(CREDENTIAL_SERVICE, CREDENTIAL_USER)
        .map_err(|err| map_store_error(err))?;
    entry
        .set_password(token.trim())
        .map_err(map_store_error)?;
    SensitiveGuard::register_secret(token.trim().to_owned());
    Ok(())
}

/// Прочитать токен из системного хранилища. `Ok(None)` — записи нет.
pub fn load_token() -> AppResult<Option<String>> {
    let entry = Entry::new(CREDENTIAL_SERVICE, CREDENTIAL_USER)
        .map_err(map_store_error)?;
    match entry.get_password() {
        Ok(token) => {
            SensitiveGuard::register_secret(token.clone());
            Ok(Some(token))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(err) => Err(map_store_error(err)),
    }
}

/// Удалить токен из системного хранилища и из списка чувствительных.
pub fn delete_token() -> AppResult<()> {
    let entry = Entry::new(CREDENTIAL_SERVICE, CREDENTIAL_USER)
        .map_err(map_store_error)?;
    match entry.delete_credential() {
        Ok(())
        | Err(keyring::Error::NoEntry) => {}
        Err(err) => return Err(map_store_error(err)),
    }
    SensitiveGuard::clear_secrets();
    Ok(())
}

/// Есть ли сохранённый токен (без чтения значения).
pub fn has_token() -> bool {
    load_token().map(|opt| opt.is_some()).unwrap_or(false)
}

fn map_store_error(_err: keyring::Error) -> AppError {
    // Никогда не включаем текст ошибки keyring целиком: он может содержать
    // частичные данные записи.
    tracing::warn!("credential store error");
    AppError::with_details(
        ErrorCode::SecretStoreError,
        "credential operation failed".to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_token_rejected_immediately() {
        let result = tokio_block(validate_token("   "));
        assert!(matches!(result, Err(AppError { code: ErrorCode::NoToken, .. })));
    }

    // Вспомогательный блок run, чтобы не включать сетевые вызовы в тесты
    // по умолчанию: сетевая логика проверяется в integration-тестах через
    // mock server.
    #[allow(dead_code)]
    fn tokio_block<F: std::future::Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fut)
    }
}
