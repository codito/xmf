//! Composite price provider that merges data from multiple ordered sources.

use crate::core::price::{PriceProvider, PriceResult};
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use std::sync::Arc;
use tracing::debug;

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

/// Ordered list of price providers. The first provider that returns a fresh
/// result becomes the base; subsequent providers fill in missing
/// `historical_prices` entries.
pub struct CompositePriceProvider {
    providers: Vec<Arc<dyn PriceProvider + Send + Sync>>,
}

impl CompositePriceProvider {
    pub fn new(providers: Vec<Arc<dyn PriceProvider + Send + Sync>>) -> Self {
        Self { providers }
    }
}

#[async_trait]
impl PriceProvider for CompositePriceProvider {
    async fn fetch_price(&self, identifier: &str) -> Result<PriceResult> {
        let mut base: Option<PriceResult> = None;

        for provider in &self.providers {
            match provider.fetch_price(identifier).await {
                Ok(result) if is_stale(&result) => {
                    debug!(
                        "Provider returned stale data for {}, trying next",
                        identifier
                    );
                    continue;
                }
                Ok(result) => {
                    match &mut base {
                        None => {
                            debug!(
                                "Using first fresh result for {} from a provider",
                                identifier
                            );
                            base = Some(result);
                        }
                        Some(b) => {
                            // Fill missing historical_periods from this provider
                            let mut filled = 0;
                            for period in result.historical_prices.keys() {
                                if !b.historical_prices.contains_key(period)
                                    && let Some(val) = result.historical_prices.get(period)
                                {
                                    b.historical_prices.insert(*period, *val);
                                    filled += 1;
                                }
                            }
                            // Fill missing short_name (e.g. MoneyControl doesn't provide fund names)
                            if b.short_name.is_none() && result.short_name.is_some() {
                                b.short_name = result.short_name;
                                filled += 1;
                            }
                            if filled > 0 {
                                debug!(
                                    "Filled {} missing fields for {} from a provider",
                                    filled, identifier
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    debug!("Provider failed for {}: {}, trying next", identifier, e);
                    continue;
                }
            }
        }

        base.ok_or_else(|| {
            anyhow::anyhow!(
                "All providers failed or returned stale data for {}",
                identifier
            )
        })
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
        fn ok(as_of: NaiveDate, price: f64) -> Self {
            Self {
                result: Arc::new(Ok(PriceResult {
                    price,
                    currency: "INR".to_string(),
                    historical_prices: HashMap::new(),
                    daily_prices: vec![(as_of, price)],
                    short_name: None,
                })),
                call_count: AtomicUsize::new(0),
            }
        }

        fn ok_with_periods(
            as_of: NaiveDate,
            price: f64,
            periods: Vec<(HistoricalPeriod, f64)>,
        ) -> Self {
            Self {
                result: Arc::new(Ok(PriceResult {
                    price,
                    currency: "INR".to_string(),
                    historical_prices: periods.into_iter().collect(),
                    daily_prices: vec![(as_of, price)],
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
    async fn test_first_fresh_provider_becomes_base() {
        let p1 = Arc::new(MockProvider::ok(days_ago(0), 100.0));
        let p2 = Arc::new(MockProvider::ok(days_ago(0), 200.0));
        let provider = CompositePriceProvider::new(vec![p1.clone(), p2.clone()]);

        let result = provider.fetch_price("X").await.unwrap();
        assert_eq!(result.price, 100.0);
        assert_eq!(p1.calls(), 1);
        assert_eq!(p2.calls(), 1);
    }

    #[tokio::test]
    async fn test_stale_primary_skips_to_fresh() {
        let p1 = Arc::new(MockProvider::ok(days_ago(STALE_AFTER_DAYS + 1), 100.0));
        let p2 = Arc::new(MockProvider::ok(days_ago(0), 200.0));
        let provider = CompositePriceProvider::new(vec![p1.clone(), p2.clone()]);

        let result = provider.fetch_price("X").await.unwrap();
        assert_eq!(result.price, 200.0);
        assert_eq!(p1.calls(), 1);
        assert_eq!(p2.calls(), 1);
    }

    #[tokio::test]
    async fn test_fills_missing_periods_from_secondary() {
        let p1 = Arc::new(MockProvider::ok_with_periods(
            days_ago(0),
            100.0,
            vec![
                (HistoricalPeriod::OneDay, 99.0),
                (HistoricalPeriod::OneMonth, 95.0),
            ],
        ));
        let p2 = Arc::new(MockProvider::ok_with_periods(
            days_ago(0),
            110.0,
            vec![
                (HistoricalPeriod::OneDay, 108.0),
                (HistoricalPeriod::FiveDays, 105.0),
                (HistoricalPeriod::OneYear, 80.0),
            ],
        ));
        let provider = CompositePriceProvider::new(vec![p1.clone(), p2.clone()]);

        let result = provider.fetch_price("X").await.unwrap();
        assert_eq!(result.price, 100.0); // base price preserved
        // OneDay from p1 (base), not overridden
        assert_eq!(
            result.historical_prices.get(&HistoricalPeriod::OneDay),
            Some(&99.0)
        );
        // OneMonth from p1 (base)
        assert_eq!(
            result.historical_prices.get(&HistoricalPeriod::OneMonth),
            Some(&95.0)
        );
        // FiveDays filled from p2
        assert_eq!(
            result.historical_prices.get(&HistoricalPeriod::FiveDays),
            Some(&105.0)
        );
        // OneYear filled from p2
        assert_eq!(
            result.historical_prices.get(&HistoricalPeriod::OneYear),
            Some(&80.0)
        );
    }

    #[tokio::test]
    async fn test_primary_failure_uses_secondary() {
        let p1 = Arc::new(MockProvider::err());
        let p2 = Arc::new(MockProvider::ok(days_ago(0), 200.0));
        let provider = CompositePriceProvider::new(vec![p1.clone(), p2.clone()]);

        let result = provider.fetch_price("X").await.unwrap();
        assert_eq!(result.price, 200.0);
    }

    #[tokio::test]
    async fn test_all_providers_fail() {
        let p1 = Arc::new(MockProvider::err());
        let p2 = Arc::new(MockProvider::err());
        let provider = CompositePriceProvider::new(vec![p1.clone(), p2.clone()]);

        let err = provider.fetch_price("X").await.unwrap_err();
        assert!(err.to_string().contains("All providers failed"));
    }

    #[tokio::test]
    async fn test_stale_primary_with_fresh_filler() {
        let p1 = Arc::new(MockProvider::ok(days_ago(STALE_AFTER_DAYS + 1), 100.0));
        let p2 = Arc::new(MockProvider::ok(days_ago(0), 200.0));
        let provider = CompositePriceProvider::new(vec![p1.clone(), p2.clone()]);

        let result = provider.fetch_price("X").await.unwrap();
        // p1 is stale, p2 is fresh → p2 becomes base
        assert_eq!(result.price, 200.0);
    }

    #[tokio::test]
    async fn test_primary_stale_secondary_also_stale() {
        let p1 = Arc::new(MockProvider::ok(days_ago(STALE_AFTER_DAYS + 1), 100.0));
        let p2 = Arc::new(MockProvider::ok(days_ago(STALE_AFTER_DAYS + 1), 200.0));
        let provider = CompositePriceProvider::new(vec![p1.clone(), p2.clone()]);

        let err = provider.fetch_price("X").await.unwrap_err();
        assert!(err.to_string().contains("All providers failed"));
    }

    #[tokio::test]
    async fn test_fills_missing_short_name_from_secondary() {
        let p1 = Arc::new(MockProvider {
            result: Arc::new(Ok(PriceResult {
                price: 100.0,
                currency: "INR".to_string(),
                historical_prices: HashMap::new(),
                daily_prices: vec![(days_ago(0), 100.0)],
                short_name: None, // no name (like MoneyControl)
            })),
            call_count: AtomicUsize::new(0),
        });
        let p2 = Arc::new(MockProvider {
            result: Arc::new(Ok(PriceResult {
                price: 110.0,
                currency: "INR".to_string(),
                historical_prices: HashMap::new(),
                daily_prices: vec![(days_ago(0), 110.0)],
                short_name: Some("HDFC Money Market Dir Gr".to_string()),
            })),
            call_count: AtomicUsize::new(0),
        });
        let provider = CompositePriceProvider::new(vec![p1.clone(), p2.clone()]);

        let result = provider.fetch_price("X").await.unwrap();
        assert_eq!(result.price, 100.0); // base price preserved
        assert_eq!(
            result.short_name,
            Some("HDFC Money Market Dir Gr".to_string())
        );
    }

    #[tokio::test]
    async fn test_does_not_override_existing_short_name() {
        let p1 = Arc::new(MockProvider {
            result: Arc::new(Ok(PriceResult {
                price: 100.0,
                currency: "INR".to_string(),
                historical_prices: HashMap::new(),
                daily_prices: vec![(days_ago(0), 100.0)],
                short_name: Some("Fund A".to_string()),
            })),
            call_count: AtomicUsize::new(0),
        });
        let p2 = Arc::new(MockProvider {
            result: Arc::new(Ok(PriceResult {
                price: 110.0,
                currency: "INR".to_string(),
                historical_prices: HashMap::new(),
                daily_prices: vec![(days_ago(0), 110.0)],
                short_name: Some("Fund B".to_string()),
            })),
            call_count: AtomicUsize::new(0),
        });
        let provider = CompositePriceProvider::new(vec![p1.clone(), p2.clone()]);

        let result = provider.fetch_price("X").await.unwrap();
        assert_eq!(result.short_name, Some("Fund A".to_string()));
    }
}
