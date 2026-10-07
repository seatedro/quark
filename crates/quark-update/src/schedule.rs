//! When the background checker runs next: a fixed interval after a
//! successful check, exponential backoff after failures.

use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    /// Between successful checks.
    pub interval: Duration,
    /// The wait after the first failure; doubles with each further failure.
    pub retry_base: Duration,
    /// The longest wait after failures.
    pub retry_max: Duration,
    failures: u32,
}

impl Default for Schedule {
    fn default() -> Self {
        Self::new(
            Duration::from_secs(6 * 60 * 60),
            Duration::from_secs(60),
            Duration::from_secs(60 * 60),
        )
    }
}

impl Schedule {
    pub fn new(interval: Duration, retry_base: Duration, retry_max: Duration) -> Self {
        Self {
            interval,
            retry_base,
            retry_max,
            failures: 0,
        }
    }

    /// The wait before the next check after one that succeeded.
    pub fn succeeded(&mut self) -> Duration {
        self.failures = 0;
        self.interval
    }

    /// The wait before retrying after a check or download that failed.
    pub fn failed(&mut self) -> Duration {
        self.failures = self.failures.saturating_add(1);
        let factor = 1u32.checked_shl(self.failures - 1).unwrap_or(u32::MAX);
        self.retry_base
            .checked_mul(factor)
            .unwrap_or(self.retry_max)
            .min(self.retry_max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_back_off_to_the_cap_and_success_resets() {
        let mut schedule = Schedule::new(
            Duration::from_secs(21_600),
            Duration::from_secs(60),
            Duration::from_secs(3_600),
        );
        let failures: Vec<u64> = (0..8).map(|_| schedule.failed().as_secs()).collect();
        assert_eq!(failures, [60, 120, 240, 480, 960, 1920, 3600, 3600]);
        // Far past the point where the doubling overflows.
        for _ in 0..100 {
            schedule.failed();
        }
        assert_eq!(schedule.failed().as_secs(), 3600);
        assert_eq!(schedule.succeeded().as_secs(), 21_600);
        assert_eq!(schedule.failed().as_secs(), 60);
    }
}
