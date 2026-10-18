// ТЗ §6.2: Application ID Discord не является секретом, но должен быть
// передан в компилируемый код. Значение берём из переменной окружения
// DISCORD_APPLICATION_ID (или .env); в debug-сборке предупреждаем о её
// отсутствии, в release — сборка падает с понятной ошибкой.
//
// Читаем `.env`: разработчик задаёт ID в корне проекта.
// Cargo и tauri CLI сами не читают `.env`, поэтому делаем это вручную.
// Реализация — простой dotenv-парсер: строки `KEY=VALUE`, комментарии
// и кавычки обрезаются.

use std::path::Path;
use std::process::exit;

/// Разбор `KEY=VALUE` из простого `.env`-файла (без вложенных кавычек/эскейпов).
/// Пустые строки и комментарии игнорируются; значения в кавычках очищаются
/// от них, как это делает dotenv-парсер.
fn parse_env_file(path: &Path) -> Vec<(String, String)> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let line = line.strip_prefix("export ").unwrap_or(line);
            let (key, value) = line.split_once('=')?;
            let key = key.trim();
            let mut value = value.trim();
            // Убираем парные обрамляющие кавычки, как это делает dotenv-парсер.
            if value.len() >= 2
                && ((value.starts_with('"') && value.ends_with('"'))
                    || (value.starts_with('\'') && value.ends_with('\'')))
            {
                value = &value[1..value.len() - 1];
            }
            if key.is_empty() {
                return None;
            }
            Some((key.to_string(), value.to_string()))
        })
        .collect()
}

fn main() {
    let is_release = std::env::var("PROFILE")
        .map(|profile| profile == "release")
        .unwrap_or(false);

    // .env ищем в текущем каталоге (src-tauri) и в корне проекта.
    let candidates = [Path::new(".env"), Path::new("../.env")];
    let mut file_id: Option<String> = None;
    for candidate in candidates {
        if candidate.exists() && file_id.is_none() {
            file_id = parse_env_file(candidate)
                .into_iter()
                .find(|(key, _)| key == "DISCORD_APPLICATION_ID")
                .map(|(_, value)| value);
        }
    }

    // Переменная окружения имеет приоритет (как у большинства dotenv).
    let id = std::env::var("DISCORD_APPLICATION_ID")
        .ok()
        .filter(|id| !id.trim().is_empty())
        .map(|id| id.trim().to_string())
        .or(file_id.map(|id| id.trim().to_string()));

    match id {
        Some(id) if !id.trim().is_empty() => {
            // Экспортируем значение в набор env-переменных (option_env! в src).
            println!("cargo:rustc-env=DISCORD_APPLICATION_ID={}", id.trim());
            // Cargo сам не отслеживает .env: без rerun-if-changed смена ID
            // не приводила к пересборке (option_env! оставался старым).
            println!("cargo:rerun-if-changed=.env");
            println!("cargo:rerun-if-changed=../.env");
        }
        _ => {
            if is_release {
                println!(
                    "\n\nDISCORD_APPLICATION_ID не задан.\n\
                     Создайте файл `.env` в корне проекта или задайте переменную окружения:\n\
                     \x20   $env:DISCORD_APPLICATION_ID=\"123456789012345678\"\n\
                     \x20   cargo tauri build\n\
                     Application ID берётся из Discord Developer Portal (это не секрет).\n"
                );
                exit(1);
            }
            println!(
                "cargo:warning=DISCORD_APPLICATION_ID не задан: Rich Presence \
                 в debug-сборке будет не публиковаться (сборка продолжится)"
            );
        }
    }

    tauri_build::build()
}
