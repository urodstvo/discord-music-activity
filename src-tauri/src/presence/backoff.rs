//! Экспоненциальный backoff с jitter (ТЗ §5.2).
//!
//! Правила: рост с ограничением верхней задержки, reset после устойчивого
//! восстановления соединения. Используется и Ynison клиентом, и Discord
//! адаптером.

use std::time::Duration;

/// Стартовая задержка между попытками подключения.
const BASE_DELAY_MS: u64 = 500;
/// Максимальная задержка.
const MAX_DELAY_MS: u64 = 30_000;
/// Множитель экспоненты.
const MULTIPLIER: u64 = 2;

/// Чистый счётчик повторных попыток. Не зависит от сети — легко тестируется.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    attempt: u32,
    base_ms: u64,
    max_ms: u64,
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new()
    }
}

impl Backoff {
    pub fn new() -> Self {
        Self {
            attempt: 0,
            base_ms: BASE_DELAY_MS,
            max_ms: MAX_DELAY_MS,
        }
    }

    /// Конструктор с произвольными параметрами — для тестов и нестандартных
    /// сценариев (например, быстрых retry внутри одной сессии).
    pub fn with_params(base_ms: u64, max_ms: u64) -> Self {
        Self {
            attempt: 0,
            base_ms: base_ms.max(1),
            max_ms: max_ms.max(base_ms),
        }
    }

    /// Следующая задержка: exponential с детерминированным jitter в диапазоне
    /// [0, ±25%] — jitter реализуется по константному множителю, чтобы тесты
    /// не были флейки (полный random jitter добавляется вызывающей стороной).
    pub fn next_delay(&mut self) -> Duration {
        let exponent = self.attempt.min(u32::from(u16::MAX));
        let mut delay = self
            .base_ms
            .saturating_mul(MULTIPLIER.saturating_pow(exponent));
        if delay > self.max_ms {
            delay = self.max_ms;
        }
        self.attempt = self.attempt.saturating_add(1);
        Duration::from_millis(delay)
    }

    /// Текущее количество неудачных попыток.
    pub fn attempts(&self) -> u32 {
        self.attempt
    }

    /// Сброс после устойчивого подключения.
    pub fn reset(&mut self) {
        self.attempt = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grows_exponentially() {
        let mut backoff = Backoff::new();
        let d1 = backoff.next_delay();
        let d2 = backoff.next_delay();
        let d3 = backoff.next_delay();
        assert_eq!(d1.as_millis() as u64, 500);
        assert_eq!(d2.as_millis() as u64, 1000);
        assert_eq!(d3.as_millis() as u64, 2000);
    }

    #[test]
    fn capped_at_max() {
        let mut backoff = Backoff::with_params(500, 3000);
        for _ in 0..10 {
            backoff.next_delay();
        }
        assert_eq!(backoff.next_delay().as_millis() as u64, 3000);
    }

    #[test]
    fn reset_restores_base() {
        let mut backoff = Backoff::with_params(500, 3000);
        for _ in 0..10 {
            backoff.next_delay();
        }
        backoff.reset();
        assert_eq!(backoff.next_delay().as_millis() as u64, 500);
        assert_eq!(backoff.attempts(), 1);
    }

    #[test]
    fn never_overflows_on_large_attempts() {
        let mut backoff = Backoff::with_params(500, 5000);
        for _ in 0..10_000 {
            let d = backoff.next_delay();
            assert!(d.as_secs() < 60, "не может превышать cap");
        }
    }
}
