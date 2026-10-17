//! Windows SMTC (System Media Transport Controls): коррекция ложной паузы
//! Ynison (референс: `correct_pause` в WinYandexMusicRPC, MIT — только
//! документированные факты, не код).
//!
//! Известный серверный глюк: Ynison иногда отдаёт `paused=true`, когда
//! устройство фактически играет, и состояние чинится лишь после
//! перерегистрации мобильного приложения. WYMR решает это сверкой с
//! реальными «теперь играет»-сессиями Windows: если какая-то SMTC-сессия
//! играет трек, совпадающий по названию, пауза считается ложной и
//! перезаписывается на Playing.
//!
//! Вызывается ТОЛЬКО для кадров со статусом Paused; блокирующие COM-вызовы
//! выполняются в выделенном потоке с общим таймаутом (см.
//! `ynison_client::pause_corrected_by_smtc`). Ошибки SMTC проглатываются:
//! отсутствие коррекции просто оставляет статус сервера.

use std::time::{Duration, Instant};

use windows::core::{Error, HRESULT, Result, RuntimeType};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus,
};
use windows_future::{AsyncStatus, IAsyncOperation};

/// Общий срок на всю SMTC-сверку (COM-вызовы идут цепочкой).
pub const SMTC_TOTAL_TIMEOUT_MS: u64 = 2_000;

/// Блокирующее ожидание WinRT async-операции (у windows-future нет
/// блокирующего `get`): poll `Status` отдаёт результат из `GetResults`.
/// По таймауту операция отменяется — поток не зависает навсегда.
fn await_smtc<T: RuntimeType + Clone>(op: IAsyncOperation<T>, deadline: Instant) -> Result<T> {
    loop {
        match op.Status() {
            Ok(AsyncStatus::Completed) => return op.GetResults(),
            Ok(AsyncStatus::Error) | Ok(AsyncStatus::Canceled) => {
                // GetResults при ошибке отдаёт HRESULT-ошибку наружу —
                // это и есть канонический способ узнать причину.
                return op.GetResults();
            }
            _ => {
                if Instant::now() >= deadline {
                    op.Cancel().ok();
                    return Err(Error::from_hresult(HRESULT(0x8000_4015_u32 as i32))); // E_ABORT
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

/// Совпадает ли играющая сейчас SMTC-сессия с треком (по названию,
/// регистронезависимо). Реализует вилку WYMR: artists-сопоставление
/// недоступно на этапе live-кадра, поэтому — строгий title-match.
pub fn smtc_reports_playing(title: &str) -> bool {
    let deadline = Instant::now() + Duration::from_millis(SMTC_TOTAL_TIMEOUT_MS);

    fn normalize(s: &str) -> String {
        s.trim().to_lowercase()
    }

    let wanted = normalize(title);
    if wanted.is_empty() {
        return false;
    }

    // RequestAsync().get() блокирует поток; вызывающая сторона обязана
    // завернуть это в spawn_blocking (см. ynison_client::dispatch_frame).
    let result = (|| -> Result<bool> {
        let manager = await_smtc(
            GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?,
            deadline,
        )?;
        let sessions = manager.GetSessions()?;
        let count = sessions.Size()?;
        for i in 0..count {
            let session = sessions.GetAt(i)?;
            let playing = session
                .GetPlaybackInfo()
                .ok()
                .and_then(|info| info.PlaybackStatus().ok())
                .is_some_and(|status| {
                    status == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing
                });
            if !playing {
                continue;
            }
        let Ok(props) = await_smtc(session.TryGetMediaPropertiesAsync()?, deadline) else {
                continue;
            };
            let session_title = props
                .Title()
                .map(|t| normalize(&t.to_string()))
                .unwrap_or_default();
            if session_title == wanted {
                return Ok(true);
            }
        }
        Ok(false)
    })();

    match result {
        Ok(found) => found,
        Err(err) => {
            // Класс ошибки без секретов. SMTC может быть недоступен
            // (нет медиа-сессий/ограничения) — коррекции просто не будет.
            tracing::debug!("SMTC check failed: {err}");
            false
        }
    }
}
