use async_trait::async_trait;
use chrono::NaiveDate;

#[derive(Debug, Clone)]
pub struct FundMetadata {
    pub isin: String,
    pub fund_type: String,
    pub fund_category: String,
    pub expense_ratio: f64,
    pub expense_ratio_date: NaiveDate,
    pub aum: f64,
    pub fund_rating: u8,
    pub fund_rating_date: NaiveDate,
    pub category: String,
}

#[async_trait]
pub trait MetadataProvider: Send + Sync {
    async fn fetch_metadata(&self, identifier: &str) -> anyhow::Result<FundMetadata>;
}
