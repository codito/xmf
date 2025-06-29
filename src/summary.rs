use crate::config::Portfolio;
use crate::currency_provider::CurrencyRateProvider; // Added import
use crate::price_provider::{PriceProvider, PriceResult};
use anyhow::{Result, anyhow};
use comfy_table::Table;
use comfy_table::modifiers::UTF8_ROUND_CORNERS;
use comfy_table::presets::UTF8_FULL;
use std::collections::HashMap;
use tracing::debug; // Added for logging

#[derive(Debug, Clone)]
pub struct InvestmentSummary {
    pub symbol: String,
    pub units: f64,
    pub current_price: Option<f64>,
    pub current_value: Option<f64>,
    pub currency: Option<String>,
    pub converted_value: Option<f64>, // Added field for converted value
    pub weight_pct: Option<f64>,
    pub error: Option<String>,
}

#[derive(Debug)]
pub struct PortfolioSummary {
    pub name: String,
    pub total_value: Option<f64>,
    pub converted_value: Option<f64>, // Added field for total converted value
    pub currency: Option<String>,
    pub investments: Vec<InvestmentSummary>,
}

impl PortfolioSummary {
    pub fn display_as_table(&self) -> String {
        let target_currency = self.currency.as_deref().unwrap_or("N/A");

        let mut table = Table::new();
        table
            .load_preset(UTF8_FULL)
            .apply_modifier(UTF8_ROUND_CORNERS)
            .set_header(vec![
                "Symbol",
                "Units",
                "Price",
                format!("Value ({target_currency})").as_ref(),
                "Weight (%)",
                "Error",
            ]);

        for investment in &self.investments {
            let units = format!("{:.2}", investment.units);
            let currency = investment.currency.as_deref().unwrap_or("N/A").to_string();
            let current_price = investment
                .current_price
                .map_or("N/A".to_string(), |p| format!("{p:.2}{currency}"));
            let converted_value = investment
                .converted_value
                .map_or("N/A".to_string(), |v| format!("{v:.2}"));
            let weight_pct = investment
                .weight_pct
                .map_or("N/A".to_string(), |w| format!("{w:.2}%"));
            let error = investment.error.as_deref().unwrap_or("").to_string();

            table.add_row(vec![
                &investment.symbol,
                &units,
                &current_price,
                &converted_value,
                &weight_pct,
                &error,
            ]);
        }

        let total_converted_value = self
            .converted_value
            .map_or("N/A".to_string(), |v| format!("{v:.2}"));

        let summary_text = format!(
            "Portfolio: {} | Total Value ({}) : {}",
            self.name, target_currency, total_converted_value
        );

        let table_output = table.to_string();
        format!("{table_output}\n\n{summary_text}")
    }
}

pub async fn generate_portfolio_summary(
    portfolio: &Portfolio,
    symbol_provider: &(dyn PriceProvider + Send + Sync),
    isin_provider: &(dyn PriceProvider + Send + Sync),
    currency_provider: &(dyn CurrencyRateProvider + Send + Sync),
    price_cache: &mut HashMap<String, Result<PriceResult, String>>,
    portfolio_currency: &str,
) -> PortfolioSummary {
    let mut summary = PortfolioSummary {
        name: portfolio.name.clone(),
        total_value: None,
        converted_value: None, // Initialize new field
        currency: Some(portfolio_currency.to_string()),
        investments: Vec::new(),
    };
    let mut portfolio_value = 0.0;
    let mut total_converted_value = 0.0; // New accumulator for converted total
    let mut all_valid = true;

    for investment in &portfolio.investments {
        // Get identifier - use ISIN if present, otherwise symbol
        // Determine which identifier to use (ISIN or symbol)
        let (identifier, provider_to_use) = if let Some(isin) = &investment.isin {
            (isin.clone(), isin_provider)
        } else if let Some(symbol) = &investment.symbol {
            (symbol.clone(), symbol_provider)
        } else {
            let invalid_investment = InvestmentSummary {
                symbol: "".to_string(),
                units: investment.units,
                current_price: None,
                current_value: None,
                converted_value: None,
                currency: None,
                weight_pct: None,
                error: Some("Investment has neither symbol nor ISIN".to_string()),
            };
            summary.investments.push(invalid_investment);
            continue;
        };

        let price_result = if let Some(cached) = price_cache.get(&identifier) {
            match cached {
                Ok(pr) => Ok(pr.clone()),
                Err(e) => Err(anyhow!(e.clone())),
            }
        } else {
            let result = provider_to_use.fetch_price(&identifier).await;
            let cache_entry = result.as_ref().map_err(|e| e.to_string());
            price_cache.insert(identifier.clone(), cache_entry.cloned());
            result
        };

        let mut investment_summary = InvestmentSummary {
            symbol: identifier.clone(),
            units: investment.units,
            current_price: None,
            current_value: None,
            converted_value: None,
            currency: None,
            weight_pct: None,
            error: None,
        };

        match price_result {
            Ok(price_data) => {
                let value = investment.units * price_data.price;
                investment_summary.current_price = Some(price_data.price);
                investment_summary.current_value = Some(value);
                investment_summary.currency = Some(price_data.currency.clone());

                // Perform currency conversion if needed
                if price_data.currency == portfolio_currency {
                    debug!(
                        "No currency conversion needed for {:?} ({} -> {})",
                        investment.symbol, price_data.currency, portfolio_currency
                    );
                    total_converted_value += value;
                    investment_summary.converted_value = Some(value);
                } else {
                    debug!(
                        "Attempting currency conversion for {:?} ({} -> {})",
                        investment.symbol, price_data.currency, portfolio_currency
                    );
                    match currency_provider
                        .get_rate(&price_data.currency, portfolio_currency)
                        .await
                    {
                        Ok(rate) => {
                            let converted_value = value * rate;
                            total_converted_value += converted_value;
                            investment_summary.converted_value = Some(converted_value);
                            debug!(
                                "Converted {} from {} to {} at rate {}: {}",
                                value,
                                price_data.currency,
                                portfolio_currency,
                                rate,
                                converted_value
                            );
                        }
                        Err(e) => {
                            investment_summary.error = Some(format!(
                                "Currency conversion failed from {} to {}: {}",
                                price_data.currency, portfolio_currency, e
                            ));
                            all_valid = false;
                            debug!(
                                "Currency conversion error for {:?}: {}",
                                investment.symbol, e
                            );
                        }
                    }
                }

                portfolio_value += value;
            }
            Err(e) => {
                all_valid = false;
                investment_summary.error = Some(e.to_string());
                debug!("Price fetch error for {:?}: {}", investment.symbol, e);
            }
        };
        summary.investments.push(investment_summary);
    }

    if all_valid {
        // Removed && portfolio_value > 0.0 condition here as total_converted_value is now key
        summary.converted_value = Some(total_converted_value);
        summary.total_value = Some(portfolio_value); // Keep original total_value
        for investment in &mut summary.investments {
            if let Some(value) = investment.converted_value {
                // Weight percentage is now based on converted_value
                investment.weight_pct = Some((value / total_converted_value) * 100.0);
            }
        }
    } else {
        // If not all valid, ensure converted_value and total_value are None
        summary.converted_value = None;
        summary.total_value = None; // Reset original total_value too if any error
    }

    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::currency_provider::CurrencyRateProvider; // Added for testing currency conversion
    use crate::{config::Investment, price_provider::PriceResult};
    use anyhow::{Result, anyhow};
    use async_trait::async_trait;
    use std::collections::HashMap;

    // MockProvider for PriceProvider remains the same
    struct MockProvider {
        responses: HashMap<String, Result<PriceResult>>,
    }

    impl MockProvider {
        fn new() -> Self {
            MockProvider {
                responses: HashMap::new(),
            }
        }

        fn add_response(&mut self, symbol: &str, price: f64, currency: &str) {
            self.responses.insert(
                symbol.to_string(),
                Ok(PriceResult {
                    price,
                    currency: currency.to_string(),
                }),
            );
        }

        fn add_error(&mut self, symbol: &str, error: &str) {
            self.responses
                .insert(symbol.to_string(), Err(anyhow!(error.to_string())));
        }
    }

    #[async_trait]
    impl PriceProvider for MockProvider {
        async fn fetch_price(&self, symbol: &str) -> Result<PriceResult> {
            match self.responses.get(symbol) {
                Some(result) => match result {
                    Ok(response) => Ok(response.clone()),
                    Err(e) => Err(anyhow!("{e:?}")),
                },
                None => Err(anyhow!("Symbol not found")),
            }
        }
    }

    // MockCurrencyProvider for CurrencyRateProvider
    struct MockCurrencyProvider {
        rates: HashMap<String, f64>,
        error_rates: HashMap<String, String>,
    }

    impl MockCurrencyProvider {
        fn new() -> Self {
            MockCurrencyProvider {
                rates: HashMap::new(),
                error_rates: HashMap::new(),
            }
        }

        fn add_rate(&mut self, from: &str, to: &str, rate: f64) {
            let key = format!("{from}:{to}");
            self.rates.insert(key, rate);
        }

        fn add_error(&mut self, from: &str, to: &str, error_msg: &str) {
            let key = format!("{from}:{to}");
            self.error_rates.insert(key, error_msg.to_string());
        }
    }

    #[async_trait]
    impl CurrencyRateProvider for MockCurrencyProvider {
        async fn get_rate(&self, from: &str, to: &str) -> Result<f64> {
            let key = format!("{from}:{to}");
            if let Some(error_msg) = self.error_rates.get(&key) {
                return Err(anyhow!(error_msg.clone()));
            }
            self.rates
                .get(&key)
                .cloned()
                .ok_or_else(|| anyhow!("Rate not found for {} to {}", from, to))
        }
    }

    fn create_portfolio(name: &str, investments: Vec<(&str, f64)>) -> Portfolio {
        Portfolio {
            name: name.to_string(),
            investments: investments
                .into_iter()
                .map(|(s, u)| Investment {
                    symbol: Some(s.to_string()),
                    isin: None,
                    units: u,
                })
                .collect(),
        }
    }

    #[tokio::test]
    async fn test_valid_single_investment() {
        let mut price_provider = MockProvider::new();
        price_provider.add_response("AAPL", 150.0, "USD");
        let isin_provider = MockProvider::new();
        let currency_provider = MockCurrencyProvider::new(); // No conversion needed, so empty mock is fine
        let mut cache = HashMap::new();
        let portfolio = create_portfolio("Tech", vec![("AAPL", 10.0)]);

        let summary = generate_portfolio_summary(
            &portfolio,
            &price_provider,
            &isin_provider,
            &currency_provider,
            &mut cache,
            "USD",
        )
        .await;

        assert_eq!(summary.name, "Tech");
        assert_eq!(summary.total_value, Some(1500.0));
        assert_eq!(summary.converted_value, Some(1500.0)); // Check converted value
        assert_eq!(summary.currency, Some("USD".to_string()));
        assert_eq!(summary.investments[0].symbol, "AAPL");
        assert_eq!(summary.investments[0].current_value, Some(1500.0));
        assert_eq!(summary.investments[0].converted_value, Some(1500.0)); // Check converted investment value
        assert_eq!(summary.investments[0].weight_pct, Some(100.0));
        assert_eq!(summary.investments[0].error, None);
    }

    #[tokio::test]
    async fn test_error_handling_price_fetch() {
        let mut price_provider = MockProvider::new();
        price_provider.add_response("AAPL", 150.0, "USD");
        price_provider.add_error("MSFT", "API unavailable");
        let isin_provider = MockProvider::new();
        let currency_provider = MockCurrencyProvider::new();
        let mut cache = HashMap::new();
        let portfolio = create_portfolio("Tech", vec![("AAPL", 10.0), ("MSFT", 5.0)]);

        let summary = generate_portfolio_summary(
            &portfolio,
            &price_provider,
            &isin_provider,
            &currency_provider,
            &mut cache,
            "USD",
        )
        .await;

        assert!(summary.total_value.is_none()); // Total value should be none if any error
        assert!(summary.converted_value.is_none()); // Converted value should be none
        assert_eq!(summary.investments[0].error, None);
        assert_eq!(
            summary.investments[1].error,
            Some("API unavailable".to_string())
        );
        assert!(summary.investments[0].converted_value.is_some()); // First investment still has converted value
        assert!(summary.investments[1].converted_value.is_none()); // Second investment has no converted value
    }

    #[tokio::test]
    async fn test_mixed_currencies_with_conversion() {
        let mut price_provider = MockProvider::new();
        price_provider.add_response("AAPL", 150.0, "USD");
        price_provider.add_response("RY", 100.0, "CAD");
        let isin_provider = MockProvider::new();
        let mut currency_provider = MockCurrencyProvider::new();
        currency_provider.add_rate("CAD", "USD", 0.75); // 1 CAD = 0.75 USD
        let mut cache = HashMap::new();
        let portfolio = create_portfolio("Diversified", vec![("AAPL", 10.0), ("RY", 10.0)]); // AAPL: $1500 USD, RY: $1000 CAD

        let summary = generate_portfolio_summary(
            &portfolio,
            &price_provider,
            &isin_provider,
            &currency_provider,
            &mut cache,
            "USD",
        )
        .await;

        // AAPL: 10 units * $150 USD = $1500 USD
        // RY: 10 units * $100 CAD * 0.75 USD/CAD = $750 USD
        // Total converted: $1500 + $750 = $2250 USD

        assert_eq!(summary.name, "Diversified");
        assert_eq!(summary.converted_value, Some(2250.0)); // Total converted value
        assert_eq!(summary.currency, Some("USD".to_string()));

        // AAPL summary
        assert_eq!(summary.investments[0].symbol, "AAPL");
        assert_eq!(summary.investments[0].current_value, Some(1500.0));
        assert_eq!(summary.investments[0].converted_value, Some(1500.0));
        assert_eq!(
            summary.investments[0].weight_pct,
            Some((1500.0 / 2250.0) * 100.0)
        );
        assert_eq!(summary.investments[0].error, None);

        // RY summary
        assert_eq!(summary.investments[1].symbol, "RY");
        assert_eq!(summary.investments[1].current_value, Some(1000.0)); // Original CAD value
        assert_eq!(summary.investments[1].converted_value, Some(750.0)); // Converted USD value
        assert_eq!(
            summary.investments[1].weight_pct,
            Some((750.0 / 2250.0) * 100.0)
        );
        assert_eq!(summary.investments[1].error, None);
    }

    #[tokio::test]
    async fn test_currency_conversion_error() {
        let mut price_provider = MockProvider::new();
        price_provider.add_response("AAPL", 150.0, "USD");
        price_provider.add_response("RY", 100.0, "CAD");
        let isin_provider = MockProvider::new();
        let mut currency_provider = MockCurrencyProvider::new();
        currency_provider.add_error("CAD", "USD", "Rate service unavailable"); // Simulate conversion error
        let mut cache = HashMap::new();
        let portfolio = create_portfolio("Diversified", vec![("AAPL", 10.0), ("RY", 10.0)]);

        let summary = generate_portfolio_summary(
            &portfolio,
            &price_provider,
            &isin_provider,
            &currency_provider,
            &mut cache,
            "USD",
        )
        .await;

        assert!(summary.converted_value.is_none()); // Total converted should be None due to error
        assert_eq!(summary.investments[0].error, None); // AAPL is fine
        assert_eq!(
            summary.investments[1].error,
            Some(
                "Currency conversion failed from CAD to USD: Rate service unavailable".to_string()
            )
        );
        assert!(summary.investments[0].converted_value.is_some()); // AAPL conversion was fine
        assert!(summary.investments[1].converted_value.is_none()); // RY conversion failed
    }

    #[tokio::test]
    async fn test_price_caching() {
        // This test needs to be slightly adjusted as `generate_portfolio_summary` now
        // takes a currency_provider. For caching, we need to ensure the mock doesn't
        // affect the price provider's caching behavior.
        let mut price_provider = MockProvider::new();
        price_provider.add_response("AAPL", 150.0, "USD");
        let isin_provider = MockProvider::new();
        let currency_provider = MockCurrencyProvider::new(); // Dummy provider for test
        let mut cache = HashMap::new();
        let portfolio1 = create_portfolio("P1", vec![("AAPL", 10.0)]);
        let portfolio2 = create_portfolio("P2", vec![("AAPL", 5.0)]);

        // First call fetches price and caches it
        generate_portfolio_summary(
            &portfolio1,
            &price_provider,
            &isin_provider,
            &currency_provider,
            &mut cache,
            "USD",
        )
        .await;
        // Second call uses cache for price
        let summary = generate_portfolio_summary(
            &portfolio2,
            &price_provider,
            &isin_provider,
            &currency_provider,
            &mut cache,
            "USD",
        )
        .await;

        assert_eq!(summary.investments[0].current_value, Some(750.0));
        assert_eq!(summary.investments[0].converted_value, Some(750.0)); // Also check converted value
        assert_eq!(cache.len(), 1); // Ensure only one item in cache
    }
}
