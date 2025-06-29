//! Provides currency rate conversion for the application.

use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait CurrencyRateProvider: Send + Sync {
    async fn get_rate(&self, from: &str, to: &str) -> Result<f64>;
}
