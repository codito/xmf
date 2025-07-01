use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HistoricalPeriod {
    OneWeek,
    OneMonth,
    OneYear,
    ThreeYears,
    FiveYears,
}

#[derive(Debug, Clone)]
pub struct PriceResult {
    pub price: f64,
    pub currency: String,
    pub historical: HashMap<HistoricalPeriod, f64>,
}

#[async_trait]
pub trait PriceProvider: Send + Sync {
    async fn fetch_price(&self, symbol: &str) -> Result<PriceResult>;
}
