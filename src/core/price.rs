//! Pricing abstractions and core types

use anyhow::Result;
use async_trait::async_trait;
use chrono::Duration;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Display;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub enum HistoricalPeriod {
    OneDay,
    FiveDays,
    OneMonth,
    OneYear,
    ThreeYears,
    FiveYears,
    TenYears,
}

impl Display for HistoricalPeriod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                HistoricalPeriod::OneDay => "1D",
                HistoricalPeriod::FiveDays => "5D",
                HistoricalPeriod::OneMonth => "1M",
                HistoricalPeriod::OneYear => "1Y",
                HistoricalPeriod::ThreeYears => "3Y",
                HistoricalPeriod::FiveYears => "5Y",
                HistoricalPeriod::TenYears => "10Y",
            }
        )
    }
}

impl HistoricalPeriod {
    pub fn to_duration(&self) -> Duration {
        match self {
            HistoricalPeriod::OneDay => Duration::days(1),
            HistoricalPeriod::FiveDays => Duration::days(5),
            HistoricalPeriod::OneMonth => Duration::days(30),
            HistoricalPeriod::OneYear => Duration::days(365),
            HistoricalPeriod::ThreeYears => Duration::days(365 * 3),
            HistoricalPeriod::FiveYears => Duration::days(365 * 5),
            HistoricalPeriod::TenYears => Duration::days(365 * 10),
        }
    }
}

impl FromStr for HistoricalPeriod {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "1D" => Ok(HistoricalPeriod::OneDay),
            "5D" => Ok(HistoricalPeriod::FiveDays),
            "1M" => Ok(HistoricalPeriod::OneMonth),
            "1Y" => Ok(HistoricalPeriod::OneYear),
            "3Y" => Ok(HistoricalPeriod::ThreeYears),
            "5Y" => Ok(HistoricalPeriod::FiveYears),
            "10Y" => Ok(HistoricalPeriod::TenYears),
            _ => Err(anyhow::anyhow!("Invalid historical period: {}", s)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceResult {
    pub price: f64,
    pub currency: String,
    pub historical_prices: HashMap<HistoricalPeriod, f64>,
    pub short_name: Option<String>,
}

#[async_trait]
pub trait PriceProvider: Send + Sync {
    async fn fetch_price(&self, symbol: &str) -> Result<PriceResult>;
}
