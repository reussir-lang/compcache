use crate::{Result, error::IoContext};
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Bounded synchronous retries for idempotent filesystem operations. This does
/// not retry producers or replay SQLite transactions with unknown outcomes.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Backoff {
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub max_elapsed_ms: u64,
    pub jitter: bool,
}
impl Default for Backoff {
    fn default() -> Self {
        Self {
            initial_delay_ms: 2,
            max_delay_ms: 64,
            max_elapsed_ms: 1500,
            jitter: true,
        }
    }
}
impl Backoff {
    pub(crate) fn run<T>(
        &self,
        action: &'static str,
        path: &Path,
        operation: impl FnMut() -> io::Result<T>,
    ) -> Result<T> {
        self.retry(operation, |error| transient(error, cfg!(windows)))
            .at(action, path)
    }
    fn retry<T>(
        &self,
        mut operation: impl FnMut() -> io::Result<T>,
        transient: impl Fn(&io::Error) -> bool,
    ) -> io::Result<T> {
        let started = Instant::now();
        let budget = Duration::from_millis(self.max_elapsed_ms);
        let mut delay_ms = self.initial_delay_ms;
        let mut attempt = 0u32;
        loop {
            match operation() {
                Ok(value) => return Ok(value),
                Err(error) => {
                    let elapsed = started.elapsed();
                    if !transient(&error) || elapsed >= budget {
                        return Err(error);
                    }
                    attempt += 1;
                    let delay = if self.jitter {
                        jitter(delay_ms)
                    } else {
                        delay_ms
                    };
                    let pause = Duration::from_millis(delay).min(budget.saturating_sub(elapsed));
                    tracing::debug!(attempt, delay_ms = pause.as_millis(), error = %error, "retrying transient filesystem operation");
                    std::thread::sleep(pause);
                    delay_ms = delay_ms.saturating_mul(2).min(self.max_delay_ms);
                }
            }
        }
    }
}
fn jitter(cap: u64) -> u64 {
    static SEQUENCE: AtomicU64 = AtomicU64::new(1);
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;
    let random = time
        .wrapping_add(SEQUENCE.fetch_add(1, Ordering::Relaxed))
        .wrapping_add(std::process::id() as u64);
    // Equal jitter: between half and all of the current exponential delay.
    let half = cap / 2;
    half + random % (cap - half + 1)
}
fn transient(error: &io::Error, windows: bool) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    ) || (windows && matches!(error.raw_os_error(), Some(5 | 32 | 33 | 145)))
    // Windows: access denied, sharing violation, lock violation, directory not empty.
    // Access denied may be permanent; the short retry budget bounds that case.
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retries_scanner_sharing_violations_but_not_arbitrary_failures() {
        let policy = Backoff {
            initial_delay_ms: 1,
            max_delay_ms: 2,
            max_elapsed_ms: 100,
            jitter: false,
        };
        let mut attempts = 0;
        let value = policy
            .retry(
                || {
                    attempts += 1;
                    if attempts < 3 {
                        Err(io::Error::from_raw_os_error(32))
                    } else {
                        Ok(42)
                    }
                },
                |e| transient(e, true),
            )
            .unwrap();
        assert_eq!(value, 42);
        assert_eq!(attempts, 3);
        assert!(!transient(&io::Error::from_raw_os_error(32), false));
        let mut attempts = 0;
        assert!(
            policy
                .retry(
                    || {
                        attempts += 1;
                        Err::<(), _>(io::Error::from_raw_os_error(2))
                    },
                    |e| transient(e, true)
                )
                .is_err()
        );
        assert_eq!(attempts, 1);
    }
    #[test]
    fn retry_budget_is_finite() {
        let policy = Backoff {
            max_elapsed_ms: 5,
            jitter: false,
            ..Default::default()
        };
        let started = Instant::now();
        assert!(
            policy
                .retry(
                    || Err::<(), _>(io::Error::from_raw_os_error(32)),
                    |e| transient(e, true)
                )
                .is_err()
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
