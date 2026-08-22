//! Composite price providers that combine multiple data sources.

use crate::core::price::{PriceProvider, PriceResult};
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use std::sync::Arc;

/// Maximum age (in days) of price data before it is considered stale.
const STALE_AFTER_DAYS: i64 = 5;

/// Returns true when the result's data is missing or older than
/// [`STALE_AFTER_DAYS`] days.
fn is_stale(result: &PriceResult) -> bool {
    match result.as_of_date() {
        Some(date) => Utc::now().date_naive() - date > chrono::Duration::days(STALE_AFTER_DAYS),
        None => true,
    }
}

/// Tries the primary provider first and falls back to the secondary one when
/// the primary data is stale or unavailable.
pub struct FallbackPriceProvider {
    primary: Arc<dyn PriceProvider + Send + Sync>,
    secondary: Arc<dyn PriceProvider + Send + Sync>,
}

impl FallbackPriceProvider {
    pub fn new(
        primary: Arc<dyn PriceProvider + Send + Sync>,
        secondary: Arc<dyn PriceProvider + Send + Sync>,
    ) -> Self {
        Self { primary, secondary }
    }
}

#[async_trait]
impl PriceProvider for FallbackPriceProvider {
    async fn fetch_price(&self, identifier: &str) -> Result<PriceResult> {
        match self.primary.fetch_price(identifier).await {
            Ok(result) if !is_stale(&result) => Ok(result),
            // Prefer the fresher secondary data; degrade to the primary
            // result when the fallback fails.
            Ok(primary_result) => match self.secondary.fetch_price(identifier).await {
                Ok(result) => Ok(result),
                Err(_) => Ok(primary_result),
            },
            Err(primary_err) => self
                .secondary
                .fetch_price(identifier)
                .await
                .map_err(|_| primary_err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::price::HistoricalPeriod;
    use anyhow::anyhow;
    use chrono::NaiveDate;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct MockProvider {
        result: Arc<anyhow::Result<PriceResult>>,
        call_count: AtomicUsize,
    }

    impl MockProvider {
        fn ok(as_of: NaiveDate) -> Self {
            Self {
                result: Arc::new(Ok(PriceResult {
                    price: 100.0,
                    currency: "INR".to_string(),
                    historical_prices: HashMap::new(),
                    daily_prices: vec![(as_of, 100.0)],
                    short_name: None,
                })),
                call_count: AtomicUsize::new(0),
            }
        }

        fn err() -> Self {
            Self {
                result: Arc::new(Err(anyhow!("provider unavailable"))),
                call_count: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl PriceProvider for MockProvider {
        async fn fetch_price(&self, _identifier: &str) -> Result<PriceResult> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            match self.result.as_ref() {
                Ok(result) => Ok(result.clone()),
                Err(err) => Err(anyhow!("{err}")),
            }
        }
    }

    fn days_ago(days: i64) -> NaiveDate {
        Utc::now().date_naive() - chrono::Duration::days(days)
    }

    #[tokio::test]
    async fn test_fresh_primary_short_circuits_secondary() {
        let primary = Arc::new(MockProvider::ok(days_ago(STALE_AFTER_DAYS))); // exactly at threshold = fresh
        let secondary = Arc::new(MockProvider::ok(days_ago(0)));
        let provider = FallbackPriceProvider::new(primary.clone(), secondary.clone());

        let result = provider.fetch_price("X").await.unwrap();
        assert_eq!(result.as_of_date(), Some(days_ago(STALE_AFTER_DAYS)));
        assert_eq!(primary.calls(), 1);
        assert_eq!(secondary.calls(), 0);
    }

    #[tokio::test]
    async fn test_stale_primary_uses_secondary() {
        let primary = Arc::new(MockProvider::ok(days_ago(STALE_AFTER_DAYS + 1)));
        let secondary = Arc::new(MockProvider::ok(days_ago(0)));
        let provider = FallbackPriceProvider::new(primary.clone(), secondary.clone());

        let result = provider.fetch_price("X").await.unwrap();
        assert_eq!(result.as_of_date(), Some(days_ago(0)));
        assert_eq!(primary.calls(), 1);
        assert_eq!(secondary.calls(), 1);
    }

    #[tokio::test]
    async fn test_stale_primary_without_date_uses_secondary() {
        let mut primary = MockProvider::ok(days_ago(0));
        primary.result = Arc::new(Ok(PriceResult {
            price: 100.0,
            currency: "INR".to_string(),
            historical_prices: HashMap::from([(HistoricalPeriod::OneDay, 99.0)]),
            daily_prices: Vec::new(), // no dates -> stale
            short_name: None,
        }));
        let secondary = Arc::new(MockProvider::ok(days_ago(0)));
        let provider = FallbackPriceProvider::new(Arc::new(primary), secondary.clone());

        let result = provider.fetch_price("X").await.unwrap();
        assert_eq!(result.daily_prices.len(), 1); // secondary has dated data
        assert_eq!(secondary.calls(), 1);
    }

    #[tokio::test]
    async fn test_secondary_failure_degrades_to_primary() {
        let primary = Arc::new(MockProvider::ok(days_ago(STALE_AFTER_DAYS + 1)));
        let secondary = Arc::new(MockProvider::err());
        let provider = FallbackPriceProvider::new(primary.clone(), secondary);

        let result = provider.fetch_price("X").await.unwrap();
        assert_eq!(result.as_of_date(), Some(days_ago(STALE_AFTER_DAYS + 1)));
        assert_eq!(primary.calls(), 1);
    }

    #[tokio::test]
    async fn test_primary_error_propagates_when_secondary_also_fails() {
        let primary = Arc::new(MockProvider::err());
        let secondary = Arc::new(MockProvider::err());
        let provider = FallbackPriceProvider::new(primary.clone(), secondary.clone());

        let err = provider.fetch_price("X").await.unwrap_err();
        assert_eq!(err.to_string(), "provider unavailable");
        assert_eq!(primary.calls(), 1);
        assert_eq!(secondary.calls(), 1);
    }
}
