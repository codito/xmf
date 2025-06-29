use anyhow::Result;
use async_trait::async_trait;

#[derive(Debug, Clone)]
pub struct PriceResult {
    pub price: f64,
    pub currency: String,
}

#[async_trait]
pub trait PriceProvider: Send + Sync {
    async fn fetch_price(&self, symbol: &str) -> Result<PriceResult>;
}
