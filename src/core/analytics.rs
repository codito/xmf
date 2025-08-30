//! Provides functions for performing financial calculations on portfolios.
use crate::core::config::{Investment, Portfolio};
use crate::core::currency::CurrencyRateProvider;
use crate::core::price::{HistoricalPeriod, PriceResult};
use anyhow::{Result, anyhow};
use std::collections::HashMap;
use tracing::debug;

/// Represents the calculated value and weight of a single investment holding.
#[derive(Debug, Clone)]
pub struct InvestmentValue {
    pub identifier: String,
    pub short_name: Option<String>,
    pub units: Option<f64>,
    pub price: Option<f64>,
    pub value: Option<f64>,
    pub value_currency: Option<String>,
    pub converted_value: Option<f64>,
    pub weight: Option<f64>,
    pub error: Option<String>,
}

/// Represents a summary of a portfolio's holdings, with all values
/// normalized to a target currency.
#[derive(Debug)]
pub struct PortfolioValue {
    pub name: String,
    pub investments: Vec<InvestmentValue>,
    pub total_converted_value: Option<f64>,
    pub target_currency: String,
}

/// Calculates the market value and weight of each investment in a portfolio.
///
/// This function normalizes all investment values into a single `target_currency`
/// to provide a consolidated view of the portfolio's holdings. It is a pure
/// calculation function. Progress updates can be reported via the `update_callback`.
pub async fn calculate_portfolio_value(
    portfolio: &Portfolio,
    price_results: &HashMap<String, Result<PriceResult>>,
    currency_provider: &(dyn CurrencyRateProvider + Send + Sync),
    target_currency: &str,
    update_callback: &(dyn Fn()),
) -> PortfolioValue {
    let mut holdings = PortfolioValue {
        name: portfolio.name.clone(),
        investments: Vec::new(),
        total_converted_value: None,
        target_currency: target_currency.to_string(),
    };
    let mut total_converted_value = 0.0;
    let mut all_valid = true;

    for investment in &portfolio.investments {
        let (identifier, units, needs_fetch, value_currency, value) = match investment {
            Investment::FixedDeposit(fd) => (
                fd.name.clone(),
                None,
                false,
                fd.currency
                    .clone()
                    .or_else(|| Some(target_currency.to_string())),
                Some(fd.value),
            ),
            Investment::Stock(s) => (s.symbol.clone(), Some(s.units), true, None, None),
            Investment::MutualFund(mf) => (mf.isin.clone(), Some(mf.units), true, None, None),
        };

        let mut holding = InvestmentValue {
            identifier: identifier.clone(),
            short_name: None,
            units,
            price: None,
            value,
            value_currency,
            converted_value: None,
            weight: None,
            error: None,
        };

        if needs_fetch {
            match price_results.get(&identifier) {
                Some(Ok(price_data)) => {
                    let value = units.unwrap() * price_data.price;
                    holding.price = Some(price_data.price);
                    holding.value = Some(value);
                    holding.value_currency = Some(price_data.currency.clone());
                    holding.short_name = price_data.short_name.clone();
                }
                Some(Err(e)) => {
                    all_valid = false;
                    holding.error = Some(e.to_string());
                    debug!("Price fetch error for {}: {}", identifier, e);
                }
                None => {
                    all_valid = false;
                    holding.error = Some(format!("Price data not available for {identifier}"));
                    debug!(
                        "Price data not found for {} in pre-fetched results map",
                        identifier
                    );
                }
            }
        }

        if holding.error.is_none() {
            let current_value = holding.value.unwrap();
            let current_currency = holding.value_currency.as_ref().unwrap();
            match convert_currency(
                currency_provider,
                &holding.identifier,
                &current_value,
                current_currency,
                target_currency,
            )
            .await
            {
                Ok(converted_value) => {
                    total_converted_value += converted_value;
                    holding.converted_value = Some(converted_value);
                }
                Err(e) => {
                    all_valid = false;
                    holding.error = Some(e.to_string());
                }
            }
        }
        holdings.investments.push(holding);
        update_callback();
    }

    if all_valid {
        holdings.total_converted_value = Some(total_converted_value);
        for investment in &mut holdings.investments {
            if let Some(value) = investment.converted_value
                && total_converted_value > 0.0
            {
                investment.weight = Some((value / total_converted_value) * 100.0);
            }
        }
    }

    holdings
}

/// Private helper to perform currency conversion for a single value.
async fn convert_currency(
    currency_provider: &(dyn CurrencyRateProvider + Send + Sync),
    identifier: &str,
    current_value: &f64,
    current_currency: &str,
    target_currency: &str,
) -> Result<f64> {
    if current_currency == target_currency {
        debug!(
            "No currency conversion needed for {identifier} ({current_currency} -> {target_currency})",
        );
        return Ok(*current_value);
    }

    debug!(
        "Attempting currency conversion for {identifier} ({current_currency} -> {target_currency})",
    );
    match currency_provider
        .get_rate(current_currency, target_currency)
        .await
    {
        Ok(rate) => {
            let converted_value = current_value * rate;
            debug!(
                "Converted {current_value} from {current_currency} to {target_currency} at rate {rate}: {converted_value}",
            );
            Ok(converted_value)
        }
        Err(e) => {
            debug!("Currency conversion error for {}: {}", identifier, e);
            Err(anyhow!(format!(
                "Currency conversion failed from {current_currency} to {target_currency}: {e}",
            )))
        }
    }
}

/// Represents the statistics of rolling returns for a specific period.
#[derive(Debug, Clone, Copy)]
pub struct RollingReturnStats {
    pub average: f64,
    pub min: f64,
    pub max: f64,
    pub std_dev: f64,
    pub distribution: [f64; 5],
}

/// Calculates rolling returns for a given set of historical prices.
pub fn calculate_rolling_returns(
    price_data: &PriceResult,
    period: HistoricalPeriod,
) -> Result<Option<RollingReturnStats>> {
    if price_data.daily_prices.is_empty() {
        return Ok(None);
    }

    let trading_days = period.to_trading_days() as usize;
    if price_data.daily_prices.len() < trading_days {
        return Ok(None);
    }

    // Sort by date to ensure chronological order
    let mut sorted_daily = price_data.daily_prices.clone();
    sorted_daily.sort_by_key(|(date, _)| *date);

    // Convert to price vector only
    let prices: Vec<f64> = sorted_daily.iter().map(|(_, price)| *price).collect();

    if prices.len() < trading_days {
        return Ok(None);
    }

    let mut returns = Vec::new();
    for window in prices.windows(trading_days) {
        let start_price = window[0];
        let end_price = window[trading_days - 1];
        if start_price > 0.0 {
            let years = (trading_days as f64) / 252.0; // 252 trading days per year
            let cagr = ((end_price / start_price).powf(1.0 / years) - 1.0) * 100.0;
            returns.push(cagr);
        }
    }

    if returns.is_empty() {
        return Ok(None);
    }

    let count = returns.len() as f64;
    let average = returns.iter().sum::<f64>() / count;
    let std_dev = (returns
        .iter()
        .map(|&val| (val - average).powi(2))
        .sum::<f64>()
        / count)
        .sqrt();
    let min = returns.iter().cloned().fold(f64::MAX, f64::min);
    let max = returns.iter().cloned().fold(f64::MIN, f64::max);

    let mut distribution = [0.0; 5];
    for &ret in &returns {
        if ret < 0.0 {
            distribution[0] += 1.0;
        } else if ret < 5.0 {
            distribution[1] += 1.0;
        } else if ret < 10.0 {
            distribution[2] += 1.0;
        } else if ret < 20.0 {
            distribution[3] += 1.0;
        } else {
            distribution[4] += 1.0;
        }
    }

    for val in &mut distribution {
        *val = (*val / count) * 100.0;
    }

    Ok(Some(RollingReturnStats {
        average,
        min,
        max,
        std_dev,
        distribution,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::{FixedDepositInvestment, Investment, Portfolio, StockInvestment};
    use crate::core::currency::CurrencyRateProvider;
    use crate::core::price::PriceResult;
    use anyhow::Result;
    use async_trait::async_trait;

    // MockCurrencyProvider for CurrencyRateProvider
    struct MockCurrencyProvider {
        rates: HashMap<String, f64>,
    }

    impl MockCurrencyProvider {
        fn new() -> Self {
            MockCurrencyProvider {
                rates: HashMap::new(),
            }
        }

        fn add_rate(&mut self, from: &str, to: &str, rate: f64) {
            let key = format!("{from}:{to}");
            self.rates.insert(key, rate);
        }
    }

    #[async_trait]
    impl CurrencyRateProvider for MockCurrencyProvider {
        async fn get_rate(&self, from: &str, to: &str) -> Result<f64> {
            let key = format!("{from}:{to}");
            self.rates
                .get(&key)
                .cloned()
                .ok_or_else(|| anyhow!("Rate not found for {} to {}", from, to))
        }
    }

    #[tokio::test]
    async fn test_valid_single_investment() {
        let currency_provider = MockCurrencyProvider::new();
        let mut price_results = HashMap::new();
        price_results.insert(
            "AAPL".to_string(),
            Ok(PriceResult {
                price: 150.0,
                currency: "USD".to_string(),
                historical_prices: HashMap::new(),
                daily_prices: Vec::new(),
                short_name: Some("Apple Inc.".to_string()),
            }),
        );

        let portfolio = Portfolio {
            name: "Tech".to_string(),
            investments: vec![Investment::Stock(StockInvestment {
                symbol: "AAPL".to_string(),
                units: 10.0,
            })],
        };
        let holdings = calculate_portfolio_value(
            &portfolio,
            &price_results,
            &currency_provider,
            "USD",
            &|| (),
        )
        .await;

        assert_eq!(holdings.name, "Tech");
        assert_eq!(holdings.total_converted_value, Some(1500.0));
        assert_eq!(holdings.target_currency, "USD");
        assert_eq!(holdings.investments[0].identifier, "AAPL");
        assert_eq!(holdings.investments[0].value, Some(1500.0));
        assert_eq!(holdings.investments[0].converted_value, Some(1500.0));
        assert_eq!(holdings.investments[0].weight, Some(100.0));
        assert_eq!(holdings.investments[0].error, None);
        assert_eq!(
            holdings.investments[0].short_name,
            Some("Apple Inc.".to_string())
        );
    }

    #[tokio::test]
    async fn test_error_handling_price_fetch() {
        let currency_provider = MockCurrencyProvider::new();
        let mut price_results = HashMap::new();
        price_results.insert(
            "AAPL".to_string(),
            Ok(PriceResult {
                price: 150.0,
                currency: "USD".to_string(),
                historical_prices: HashMap::new(),
                daily_prices: Vec::new(),
                short_name: Some("Apple Inc.".to_string()),
            }),
        );
        price_results.insert("MSFT".to_string(), Err(anyhow!("API unavailable")));

        let portfolio = Portfolio {
            name: "Tech".to_string(),
            investments: vec![
                Investment::Stock(StockInvestment {
                    symbol: "AAPL".to_string(),
                    units: 10.0,
                }),
                Investment::Stock(StockInvestment {
                    symbol: "MSFT".to_string(),
                    units: 5.0,
                }),
            ],
        };

        let holdings = calculate_portfolio_value(
            &portfolio,
            &price_results,
            &currency_provider,
            "USD",
            &|| (),
        )
        .await;

        assert!(holdings.total_converted_value.is_none());
        assert_eq!(holdings.investments[0].error, None);
        assert_eq!(
            holdings.investments[1].error.as_deref(),
            Some("API unavailable")
        );
        assert!(holdings.investments[0].converted_value.is_some());
        assert!(holdings.investments[1].converted_value.is_none());
    }

    #[tokio::test]
    async fn test_mixed_currencies_with_conversion() {
        let mut price_results = HashMap::new();
        price_results.insert(
            "AAPL".to_string(),
            Ok(PriceResult {
                price: 150.0,
                currency: "USD".to_string(),
                historical_prices: HashMap::new(),
                daily_prices: Vec::new(),
                short_name: Some("Apple Inc.".to_string()),
            }),
        );
        price_results.insert(
            "RY".to_string(),
            Ok(PriceResult {
                price: 100.0,
                currency: "CAD".to_string(),
                historical_prices: HashMap::new(),
                daily_prices: Vec::new(),
                short_name: Some("Royal Bank".to_string()),
            }),
        );
        let mut currency_provider = MockCurrencyProvider::new();
        currency_provider.add_rate("CAD", "USD", 0.75);
        let portfolio = Portfolio {
            name: "Diversified".to_string(),
            investments: vec![
                Investment::Stock(StockInvestment {
                    symbol: "AAPL".to_string(),
                    units: 10.0,
                }),
                Investment::Stock(StockInvestment {
                    symbol: "RY".to_string(),
                    units: 10.0,
                }),
            ],
        };

        let holdings = calculate_portfolio_value(
            &portfolio,
            &price_results,
            &currency_provider,
            "USD",
            &|| (),
        )
        .await;

        assert_eq!(holdings.total_converted_value, Some(2250.0));
        assert_eq!(holdings.investments[0].identifier, "AAPL");
        assert_eq!(holdings.investments[0].converted_value, Some(1500.0));
        assert_eq!(
            holdings.investments[0].weight,
            Some((1500.0 / 2250.0) * 100.0)
        );
        assert_eq!(holdings.investments[1].identifier, "RY");
        assert_eq!(holdings.investments[1].value, Some(1000.0));
        assert_eq!(holdings.investments[1].converted_value, Some(750.0));
        assert_eq!(
            holdings.investments[1].weight,
            Some((750.0 / 2250.0) * 100.0)
        );
    }

    #[tokio::test]
    async fn test_fixed_deposit_investment() {
        let price_results: HashMap<String, Result<PriceResult>> = HashMap::new();
        let currency_provider = MockCurrencyProvider::new();

        let portfolio = Portfolio {
            name: "Bank".to_string(),
            investments: vec![Investment::FixedDeposit(FixedDepositInvestment {
                name: "My FD".to_string(),
                value: 5000.0,
                currency: Some("INR".to_string()),
            })],
        };

        let holdings = calculate_portfolio_value(
            &portfolio,
            &price_results,
            &currency_provider,
            "INR",
            &|| (),
        )
        .await;

        assert_eq!(holdings.total_converted_value, Some(5000.0));
        assert_eq!(holdings.investments.len(), 1);
        assert_eq!(holdings.investments[0].identifier, "My FD");
        assert_eq!(holdings.investments[0].converted_value, Some(5000.0));
        assert_eq!(holdings.investments[0].weight, Some(100.0));
    }
}
