use anyhow::Error;
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
