use async_trait::async_trait;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FundMetadata {
    pub isin: String,
    pub fund_type: String,     // Type of the fund: Debt, Equity etc.
    pub fund_category: String, // Category within a type. E.g., Liquid Fund.
    pub expense_ratio: f64,
    pub expense_ratio_date: NaiveDate,
    pub aum: f64,
    pub fund_rating: Option<u8>,
    pub fund_rating_date: Option<NaiveDate>,
    pub category: String,
}

#[async_trait]
pub trait MetadataProvider: Send + Sync {
    async fn fetch_metadata(&self, identifier: &str) -> anyhow::Result<FundMetadata>;
}
