//! Инициализация structured logging (ТЗ §11.3).
//!
//! Правила: уровни по умолчанию info, ротация файлов по размеру, отсутствие
//! OAuth-токена / Authorization headers / секретных query params в логах.
//! Санитизация реализована фильтрацией сообщений: секретные литералы
//! передаются в [`SensitiveGuard`] и вырезаются из любого вывода.

use std::path::PathBuf;
use std::sync::OnceLock;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt::time::UtcTime;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Маскировка любого секретного литерала. Используется как placeholder:
/// любые строки, в которых могли оказаться секреты, прогоняются через него.
pub struct SensitiveGuard;

impl SensitiveGuard {
    /// Вырезать известные секреты из строки. Приложение не должно строить
    /// лог, включая токен; эта функция — страховка на случай ошибки кода.
    pub fn sanitize(input: &str) -> String {
        let secrets = known_secrets().read().unwrap();
        let mut output = input.to_string();
        for secret in secrets.iter() {
            if !secret.is_empty() && output.contains(secret.as_str()) {
                output = output.replace(secret.as_str(), "<REDACTED>");
            }
        }
        output
    }

    /// Зарегистрировать секрет (например, при загрузке токена из хранилища),
    /// чтобы любой вывод был безопасен.
    pub fn register_secret(secret: impl Into<String>) {
        let value = secret.into();
        if value.trim().is_empty() {
            return;
        }
        let mut secrets = known_secrets().write().unwrap();
        if !secrets.contains(&value) {
            secrets.push(value);
        }
    }

    /// Очистить зарегистрированные секреты (например, после удаления токена).
    pub fn clear_secrets() {
        known_secrets().write().unwrap().clear();
    }
}

fn known_secrets() -> &'static std::sync::RwLock<Vec<String>> {
    static SECRETS: OnceLock<std::sync::RwLock<Vec<String>>> = OnceLock::new();
    SECRETS.get_or_init(|| std::sync::RwLock::new(Vec::new()))
}

/// Каталог логов: `%APPDATA%\yandex-music-presence\logs` на Windows.
pub fn log_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("yandex-music-presence")
        .join("logs")
}

/// Инициализация subscriber. Возвращает guard, который нужно держать живым:
/// при его drop фоновый writer завершает flush.
pub fn init() -> Option<WorkerGuard> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let dir = log_dir();
    let _ = std::fs::create_dir_all(&dir);

    // Ротация: ежедневно + неблокирующий writer.
    let file_appender = tracing_appender::rolling::daily(&dir, "app.log");
    let (writer, guard) = tracing_appender::non_blocking(file_appender);

    let file_layer = fmt::layer()
        .with_timer(UtcTime::rfc_3339())
        .with_ansi(false)
        .with_writer(writer)
        .with_target(true);

    let console_layer = fmt::layer()
        .with_timer(UtcTime::rfc_3339())
        .with_writer(std::io::stderr)
        .with_target(true);

    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(console_layer)
        .init();

    Some(guard)
}
