//! Политика свежестиPlayback состояния (ТЗ §6.6).
//!
//! При кратковременной потере соединения Ynison последняя активность
//! держится grace period, чтобы Presence не мигал при быстром reconnect.
//! После grace — очистка: «Нет актуальных данных».

use std::time::{Duration, Instant};

/// Grace period (ТЗ §6.6: целевой ориентир 10 секунд).
pub const GRACE_PERIOD: Duration = Duration::from_secs(10);

/// Чистая политика: легко покрывается тестами без сети.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GracePolicy {
    pub duration: Duration,
}

impl Default for GracePolicy {
    fn default() -> Self {
        Self::standard()
    }
}

impl GracePolicy {
    /// Целевой grace период (§6.6).
    pub fn standard() -> Self {
        Self {
            duration: GRACE_PERIOD,
        }
    }

    /// Определить, нужно ли очистить Presence: если последнее валидное
    /// состояние старше grace, очистка обязательна (ТЗ §6.6).
    pub fn is_stale(&self, last_valid: Option<Instant>, now: Instant) -> bool {
        match last_valid {
            Some(when) => now.duration_since(when) > self.duration,
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_disconnect_keeps_presence() {
        let policy = GracePolicy::standard();
        let now = Instant::now();
        // Отключение 5 секунд назад — держать Presence.
        assert!(!policy.is_stale(Some(now - Duration::from_secs(5)), now));
    }

    #[test]
    fn long_disconnect_clears_presence() {
        let policy = GracePolicy::standard();
        let now = Instant::now();
        // Отключение 15 секунд назад — очистить Presence.
        assert!(policy.is_stale(Some(now - Duration::from_secs(15)), now));
    }

    #[test]
    fn never_connected_clears_immediately() {
        let policy = GracePolicy::standard();
        assert!(policy.is_stale(None, Instant::now()));
    }
}
