use crate::currency_provider::{CurrencyRateProvider, Result as CurrencyResult};
use crate::price_provider::{PriceProvider, PriceResult, Result as PriceResultGen};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::debug;

// Caching for PriceProvider
#[derive(Clone)]
pub struct CachingPriceProvider<T: PriceProvider> {
    inner: T,
    cache: Arc<Mutex<HashMap<String, Result<PriceResult, String>>>>,
}

impl<T: PriceProvider> CachingPriceProvider<T> {
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl<T: PriceProvider + Send + Sync> PriceProvider for CachingPriceProvider<T> {
    async fn fetch_price(&self, symbol: &str) -> Result<PriceResult> {
        let mut cache = self.cache.lock().await;
        if let Some(cached_result) = cache.get(symbol) {
            debug!("Cache hit for price: {}", symbol);
            return match cached_result {
                Ok(price_result) => Ok(price_result.clone()),
                Err(e) => Err(anyhow!(e.clone())),
            };
        }
        debug!("Cache miss for price: {}", symbol);
        let result = self.inner.fetch_price(symbol).await;
        cache.insert(
            symbol.to_string(),
            result.clone().map_err(|e| e.to_string()),
        );
        result
    }
}

// Caching for CurrencyRateProvider
#[derive(Clone)]
pub struct CachingCurrencyRateProvider<T: CurrencyRateProvider> {
    inner: T,
    cache: Arc<Mutex<HashMap<String, Result<f64, String>>>>,
}

impl<T: CurrencyRateProvider> CachingCurrencyRateProvider<T> {
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl<T: CurrencyRateProvider + Send + Sync> CurrencyRateProvider for CachingCurrencyRateProvider<T> {
    async fn get_rate(&self, from: &str, to: &str) -> Result<f64> {
        let key = format!("{from}-{to}");
        let mut cache = self.cache.lock().await;
        if let Some(cached_result) = cache.get(&key) {
            debug!("Cache hit for currency rate: {}", key);
            return match cached_result {
                Ok(rate) => Ok(*rate),
                Err(e) => Err(anyhow!(e.clone())),
            };
        }
        debug!("Cache miss for currency rate: {}", key);
        let result = self.inner.get_rate(from, to).await;
        cache.insert(key, result.map_err(|e| e.to_string()));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::price_provider::{PriceProvider, PriceResult};
    use anyhow::anyhow;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct MockInnerProvider {
        call_count: AtomicUsize,
    }

    impl MockInnerProvider {
        fn new() -> Self {
            Self {
                call_count: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl<'a> PriceProvider for &'a MockInnerProvider {
        async fn fetch_price(&self, symbol: &str) -> Result<PriceResult> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            if symbol == "AAPL" {
                Ok(PriceResult {
                    price: 150.0,
                    currency: "USD".to_string(),
                    historical: HashMap::new(),
                })
            } else {
                Err(anyhow!("Unknown symbol"))
            }
        }
    }

    #[tokio::test]
    async fn test_caching_price_provider() {
        let inner_provider = MockInnerProvider::new();
        let caching_provider = CachingPriceProvider::new(&inner_provider);

        // First call - should hit inner provider
        let result1 = caching_provider.fetch_price("AAPL").await.unwrap();
        assert_eq!(result1.price, 150.0);
        assert_eq!(inner_provider.call_count.load(Ordering::SeqCst), 1);

        // Second call - should be cached
        let result2 = caching_provider.fetch_price("AAPL").await.unwrap();
        assert_eq!(result2.price, 150.0);
        assert_eq!(inner_provider.call_count.load(Ordering::SeqCst), 1);

        // Call with different symbol
        let _ = caching_provider.fetch_price("GOOG").await;
        assert_eq!(inner_provider.call_count.load(Ordering::SeqCst), 2);

        // Call again with different symbol
        let _ = caching_provider.fetch_price("GOOG").await;
        assert_eq!(inner_provider.call_count.load(Ordering::SeqCst), 2);
    }
}
