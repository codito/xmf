//! Pricing abstractions and core types

use anyhow::Result;
use async_trait::async_trait;
use chrono::{Duration, NaiveDate};
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

    pub fn to_trading_days(&self) -> u32 {
        match self {
            HistoricalPeriod::OneDay => 1,
            HistoricalPeriod::FiveDays => 5,
            HistoricalPeriod::OneMonth => 21,
            HistoricalPeriod::OneYear => 252,
            HistoricalPeriod::ThreeYears => 756,
            HistoricalPeriod::FiveYears => 1260,
            HistoricalPeriod::TenYears => 2520,
        }
    }

    pub fn variants() -> [&'static str; 7] {
        ["1D", "5D", "1M", "1Y", "3Y", "5Y", "10Y"]
    }
}

impl FromStr for HistoricalPeriod {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s_upper = s.to_uppercase();
        match s_upper.as_str() {
            "1D" => Ok(HistoricalPeriod::OneDay),
            "5D" => Ok(HistoricalPeriod::FiveDays),
            "1M" => Ok(HistoricalPeriod::OneMonth),
            "1Y" => Ok(HistoricalPeriod::OneYear),
            "3Y" => Ok(HistoricalPeriod::ThreeYears),
            "5Y" => Ok(HistoricalPeriod::FiveYears),
            "10Y" => Ok(HistoricalPeriod::TenYears),
            _ => {
                let valid_periods = Self::variants().join(", ");
                Err(anyhow::anyhow!(
                    "Invalid period: '{}'. Valid periods are: {}",
                    s,
                    valid_periods
                ))
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceResult {
    pub price: f64,
    pub currency: String,
    pub historical_prices: HashMap<HistoricalPeriod, f64>,
    pub daily_prices: Vec<(NaiveDate, f64)>,
    pub short_name: Option<String>,
}

impl PriceResult {
    pub fn as_of_date(&self) -> Option<NaiveDate> {
        self.daily_prices.iter().map(|(d, _)| d).max().copied()
    }
}

#[async_trait]
pub trait PriceProvider: Send + Sync {
    async fn fetch_price(&self, symbol: &str) -> Result<PriceResult>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_as_of_date_with_daily_prices() {
        let result = PriceResult {
            price: 150.0,
            currency: "USD".to_string(),
            historical_prices: HashMap::new(),
            daily_prices: vec![
                (NaiveDate::from_ymd_opt(2026, 6, 25).unwrap(), 145.0),
                (NaiveDate::from_ymd_opt(2026, 6, 26).unwrap(), 148.0),
                (NaiveDate::from_ymd_opt(2026, 6, 27).unwrap(), 150.0),
            ],
            short_name: None,
        };
        assert_eq!(
            result.as_of_date(),
            Some(NaiveDate::from_ymd_opt(2026, 6, 27).unwrap())
        );
    }

    #[test]
    fn test_as_of_date_empty() {
        let result = PriceResult {
            price: 150.0,
            currency: "USD".to_string(),
            historical_prices: HashMap::new(),
            daily_prices: Vec::new(),
            short_name: None,
        };
        assert_eq!(result.as_of_date(), None);
    }

    #[test]
    fn test_as_of_date_unsorted() {
        let result = PriceResult {
            price: 150.0,
            currency: "USD".to_string(),
            historical_prices: HashMap::new(),
            daily_prices: vec![
                (NaiveDate::from_ymd_opt(2026, 6, 27).unwrap(), 150.0),
                (NaiveDate::from_ymd_opt(2026, 6, 25).unwrap(), 145.0),
            ],
            short_name: None,
        };
        assert_eq!(
            result.as_of_date(),
            Some(NaiveDate::from_ymd_opt(2026, 6, 27).unwrap())
        );
    }
}
