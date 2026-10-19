//! Загрузка/сохранение настроек на диск (ТЗ §8).
//!
//! Требования: атомарная запись (temp + rename), устойчивость к битому
//! конфигу, отсутствие секретов в файле.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::config::model::AppSettings;
use crate::yandex::errors::{AppError, AppResult, ErrorCode};

#[cfg(test)]
use tempfile::TempDir; // в тестах создаём временный каталог

/// Возвращает путь к каталогу конфигурации приложения.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = dirs::config_dir() {
        dir.join("yandex-music-presence")
    } else {
        PathBuf::from(".")
    }
}

/// Полный путь к файлу настроек.
pub fn settings_path() -> PathBuf {
    config_dir().join("settings.json")
}

/// Идентификатор устройства Ynison: стабильный hex на установку, хранится
/// несекретно рядом с настройками. Используется как «Ynison-Device-Id».
pub fn load_or_create_device_id() -> String {
    let path = config_dir().join("device_id");
    if let Ok(text) = fs::read_to_string(&path) {
        let id = text.trim().to_string();
        if id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return id;
        }
    }
    // Лёгкий генератор: 16 случайных байт → 32 hex-символа.
    let mut buf = [0u8; 16];
    for (i, chunk) in buf.chunks_mut(8).enumerate() {
        let nonce = crate::rand_between(u64::MAX) ^ ((i as u64) << 33);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let mut x = nonce ^ now.rotate_left(17) ^ 0x9E3779B97F4A7C15;
        for (j, slot) in chunk.iter_mut().enumerate() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *slot = ((x >> (j * 8)) & 0xFF) as u8;
        }
    }
    let id: String = buf.iter().map(|b| format!("{b:02X}")).collect();
    // Каталог может отсутствовать при первом запуске.
    let _ = fs::create_dir_all(path.parent().unwrap_or(Path::new(".")));
    let _ = fs::write(&path, &id);
    id
}

/// Загрузить настройки. Битый или отсутствующий файл не должен ломать запуск:
/// возвращаем дефолт с трассировкой.
pub fn load_settings(path: &Path) -> AppSettings {
    match fs::read_to_string(path) {
        Ok(content) => match serde_json::from_str::<AppSettings>(&content) {
            Ok(settings) => settings.sanitized(),
            Err(err) => {
                tracing::warn!("настройки повреждены, использую defaults: {err}");
                AppSettings::default()
            }
        },
        Err(_) => AppSettings::default(),
    }
}

/// Атомарно сохранить настройки: пишем во временный файл и переименовываем.
pub fn save_settings(path: &Path, settings: &AppSettings) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            AppError::with_details(ErrorCode::SettingsReadWriteError, err.to_string())
        })?;
    }

    let tmp_path = path.with_extension("json.tmp");
    {
        let file = fs::File::create(&tmp_path).map_err(|err| {
            AppError::with_details(ErrorCode::SettingsReadWriteError, err.to_string())
        })?;
        let mut writer = std::io::BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, settings).map_err(|err| {
            AppError::with_details(ErrorCode::SettingsReadWriteError, err.to_string())
        })?;
        writer.flush().map_err(|err| {
            AppError::with_details(ErrorCode::SettingsReadWriteError, err.to_string())
        })?;
    }

    // Атомарный replace на Windows: rename поверх существующего разрешён
    // начиная с Win10; для устойчивости сначала удаляем старый файл.
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    fs::rename(&tmp_path, path).map_err(|err| {
        AppError::with_details(ErrorCode::SettingsReadWriteError, err.to_string())
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::Theme;

    fn tmp_dir() -> TempDir {
        TempDir::new().unwrap()
    }

    #[test]
    fn missing_settings_gives_defaults() {
        let dir = tmp_dir();
        let path = dir.path().join("settings.json");
        let loaded = load_settings(&path);
        assert_eq!(loaded, AppSettings::default());
    }

    #[test]
    fn roundtrip_persists_all_fields() {
        let dir = tmp_dir();
        let path = dir.path().join("settings.json");
        let settings = AppSettings {
            presence_enabled: false,
            theme: Theme::Dark,
            ..AppSettings::default()
        };
        save_settings(&path, &settings).unwrap();
        let loaded = load_settings(&path);
        assert_eq!(loaded, settings);
    }

    #[test]
    fn broken_json_falls_back_to_defaults() {
        let dir = tmp_dir();
        let path = dir.path().join("settings.json");
        fs::write(&path, "{not json").unwrap();
        let loaded = load_settings(&path);
        assert_eq!(loaded, AppSettings::default());
    }

    #[test]
    fn save_creates_parent_dirs() {
        let dir = tmp_dir();
        let path = dir.path().join("nested/subdir/settings.json");
        save_settings(&path, &AppSettings::default()).unwrap();
        assert!(path.exists());
    }
}
