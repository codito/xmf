use anyhow::{Error, Result, anyhow};
use std::future::Future;
use std::time::Duration;
use tracing::debug;

/// Retries an async operation with configurable attempts and delays
///
/// # Parameters
/// - `operation`: Closure returning a future
/// - `retries`: Number of retry attempts (total runs = 1 initial + retries)
/// - `delay_ms`: Milliseconds between retry attempts
///
/// # Returns
/// Either the successful result or the error after all attempts
pub async fn with_retry<F, Fut, T>(
    mut operation: F,
    retries: usize,
    delay_ms: u64,
) -> Result<T, Error>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, reqwest::Error>>,
{
    let mut attempt = 1;
    loop {
        match operation().await.map_err(anyhow::Error::from) {
            Ok(val) => return Ok(val),
            Err(err) => {
                if attempt > retries {
                    return Err(err);
                }
                debug!(
                    "Attempt {}/{} failed: {}. Retrying...",
                    attempt, retries, err
                );
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
        }
    }
}

/// Calculates seconds until target UTC time (hour 0-23, minute 0-59).
pub fn seconds_until(target_hour: u32, target_minute: u32) -> anyhow::Result<u64> {
    seconds_until_with_now(target_hour, target_minute, chrono::Utc::now())
}

/// Inner implementation that accepts an explicit `now` for tests.
#[inline(always)]
fn seconds_until_with_now(
    target_hour: u32,
    target_minute: u32,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<u64> {
    let mut target = now
        .date_naive()
        .and_hms_opt(target_hour, target_minute, 0)
        .ok_or_else(|| anyhow!("Invalid target time"))?
        .and_utc();

    // If target time has already passed today, schedule for tomorrow
    if target <= now {
        target += chrono::Duration::days(1);
    }

    let seconds = (target - now).num_seconds();
    if seconds < 0 {
        Err(anyhow!("Negative duration calculation"))
    } else {
        Ok(seconds as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    #[test]
    fn test_seconds_until_future_time() {
        let now = Utc.with_ymd_and_hms(2025, 1, 1, 12, 0, 0).unwrap();
        let result = seconds_until_with_now(15, 0, now).unwrap();
        assert_eq!(result, 3 * 60 * 60);
    }

    #[test]
    fn test_seconds_until_past_time() {
        let now = Utc.with_ymd_and_hms(2025, 1, 1, 18, 0, 0).unwrap();
        let result = seconds_until_with_now(10, 0, now).unwrap();
        assert_eq!(result, 16 * 60 * 60);
    }

    #[test]
    fn test_seconds_until_current_time() {
        let now = Utc.with_ymd_and_hms(2025, 1, 1, 19, 0, 0).unwrap();
        let result = seconds_until_with_now(19, 0, now).unwrap();
        assert_eq!(result, 24 * 60 * 60);
    }

    #[test]
    fn test_seconds_until_boundary() {
        let now = Utc.with_ymd_and_hms(2025, 1, 1, 18, 59, 59).unwrap();
        let result = seconds_until_with_now(19, 0, now).unwrap();
        assert_eq!(result, 1);
    }

    #[test]
    fn test_invalid_time() {
        assert!(seconds_until(24, 0).is_err());
        assert!(seconds_until(12, 60).is_err());
    }
}
