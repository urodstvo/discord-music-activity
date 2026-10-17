// Требование ТЗ §6.2: Application ID Discord должен быть зафиксирован одним
// параметром конфигурации. Он задаётся переменной окружения
// DISCORD_APPLICATION_ID (не секрет); в debug-сборке отсутствие даёт
// предупреждение, в release — понятную ошибку сборки.
//
// Плюс поддержка `.env`: пользователю удобно держать ID в корне проекта.
// Cargo и tauri CLI сами не читают `.env`, поэтому парсим его здесь.
// Приоритет — как у dotenv-подобья: уже заданная переменная окружения
// всегда сильнее значения из файла.

use std::path::Path;
use std::process::exit;

/// Считать `KEY=VALUE` из простого `.env`-файла (без кавычек/экспортов).
/// Возвращает найденные пары; невалидные строки пропускаем молча —
/// файл не источник ошибок сборки.
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
            // Снимаем одну пару кавычек, если пользователь взял значение в них.
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

    // .env ищем рядом со сборкой (src-tauri) и в корне проекта.
    let candidates = [Path::new(".env"), Path::new("../.env")];
    let mut file_id: Option<String> = None;
    for candidate in candidates {
        if Path::new(&candidate).exists() && file_id.is_none() {
            file_id = parse_env_file(candidate)
                .into_iter()
                .find(|(key, _)| key == "DISCORD_APPLICATION_ID")
                .map(|(_, value)| value);
        }
    }

    // Окружение сильнее файла (стандартная семантика dotenv).
    let id = std::env::var("DISCORD_APPLICATION_ID")
        .ok()
        .filter(|id| !id.trim().is_empty())
        .map(|id| id.trim().to_string())
        .or(file_id.map(|id| id.trim().to_string()));

    match id {
        Some(id) if !id.trim().is_empty() => {
            // Пробрасываем значение в код как env-константу (option_env! в src).
            println!("cargo:rustc-env=DISCORD_APPLICATION_ID={}", id.trim());
        }
        _ => {
            if is_release {
                println!(
                    "\n\nDISCORD_APPLICATION_ID не задан.\n\
                     Задайте переменную окружения или добавьте её в `.env`:\
                     \x20   $env:DISCORD_APPLICATION_ID=\"123456789012345678\"\
                     \x20   cargo tauri build\
                     Application ID создаётся в Discord Developer Portal (доступен всем).\n"
                );
                exit(1);
            }
            println!(
                "cargo:warning=DISCORD_APPLICATION_ID не задан: публикация Presence \
                 в debug-сборке будет возвращать ошибку конфигурации"
            );
        }
    }

    tauri_build::build()
}
