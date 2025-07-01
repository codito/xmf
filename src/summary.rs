use crate::config::Portfolio;
use crate::currency_provider::CurrencyRateProvider;
use crate::price_provider::PriceProvider;
use crate::ui;
use anyhow::Result;
use comfy_table::Cell;
use console::style;
use futures::future::join_all;
use indicatif::ProgressBar;
use tracing::debug;

#[derive(Debug, Clone)]
pub struct InvestmentSummary {
    pub symbol: String,
    pub units: Option<f64>,
    pub current_price: Option<f64>,
    pub current_value: Option<f64>,
    pub currency: Option<String>,
    pub converted_value: Option<f64>,
    pub weight_pct: Option<f64>,
    pub error: Option<String>,
}

#[derive(Debug)]
pub struct PortfolioSummary {
    pub name: String,
    pub total_value: Option<f64>,
    pub converted_value: Option<f64>,
    pub currency: Option<String>,
    pub investments: Vec<InvestmentSummary>,
}

impl PortfolioSummary {
    pub fn display_as_table(&self) -> String {
        let target_currency = self.currency.as_deref().unwrap_or("N/A");

        let mut table = ui::new_styled_table();

        table.set_header(vec![
            ui::header_cell("Symbol"),
            ui::header_cell("Units"),
            ui::header_cell("Price"),
            ui::header_cell(&format!("Value ({target_currency})")),
            ui::header_cell("Weight (%)"),
        ]);

        for investment in &self.investments {
            let currency = investment.currency.as_deref().unwrap_or("N/A").to_string();

            let units = ui::format_optional_cell(investment.units, |u| format!("{u:.2}"));
            let current_price =
                ui::format_optional_cell(investment.current_price, |p| format!("{p:.2}{currency}"));
            let converted_value =
                ui::format_optional_cell(investment.converted_value, |v| format!("{v:.2}"));
            let weight_pct =
                ui::format_optional_cell(investment.weight_pct, |w| format!("{w:.2}%"));

            table.add_row(vec![
                Cell::new(&investment.symbol),
                units,
                current_price,
                converted_value,
                weight_pct,
            ]);
        }

        let total_style_type = if self.converted_value.is_some() {
            ui::StyleType::TotalValue
        } else {
            ui::StyleType::Error
        };
        let total_converted_value = self
            .converted_value
            .map_or("N/A".to_string(), |v| format!("{v:.2}"));

        // Portfolio name at top
        let mut output = format!(
            "Portfolio: {}\n\n",
            ui::style_text(&self.name, ui::StyleType::Title)
        );

        // Table in the middle
        output.push_str(&table.to_string());

        // Total value at bottom
        output.push_str(&format!(
            "\n\nTotal Value ({}): {}",
            ui::style_text(target_currency, ui::StyleType::TotalLabel),
            ui::style_text(&total_converted_value, total_style_type)
        ));

        output
    }
}

pub async fn generate_and_display_summaries(
    portfolios: &[Portfolio],
    symbol_provider: &(dyn PriceProvider + Send + Sync),
    isin_provider: &(dyn PriceProvider + Send + Sync),
    currency_provider: &(dyn CurrencyRateProvider + Send + Sync),
    target_currency: &str,
) -> Result<()> {
    // Step 1: Generate summaries for each portfolio concurrently
    let total_investments: u64 = portfolios
        .iter()
        .map(|p| p.investments.len())
        .sum::<usize>() as u64;
    let pb = ui::new_progress_bar(total_investments, true);
    pb.set_message("Processing investments...");

    let summary_futures = portfolios.iter().map(|portfolio| {
        let pb_clone = pb.clone();
        async move {
            generate_portfolio_summary(
                portfolio,
                symbol_provider,
                isin_provider,
                currency_provider,
                target_currency,
                pb_clone,
            )
            .await
        }
    });

    let summaries = join_all(summary_futures).await;
    pb.finish_and_clear();

    // Step 2: Calculate grand total and display summaries
    let mut grand_total = 0.0;
    let mut all_portfolios_valid = true;

    for sum in &summaries {
        if let Some(value) = sum.converted_value {
            grand_total += value;
        } else {
            all_portfolios_valid = false;
        }
    }

    let num_summaries = summaries.len();
    for (i, sum) in summaries.into_iter().enumerate() {
        println!("{}", sum.display_as_table());
        if i < num_summaries - 1 {
            ui::print_separator();
        }
    }

    if all_portfolios_valid && num_summaries > 1 {
        let term_width = console::Term::stdout()
            .size_checked()
            .map(|(_, w)| w as usize)
            .unwrap_or(80);
        println!("\n{}", "=".repeat(term_width));
        let total_str = format!("Grand Total ({target_currency}): {grand_total:.2}");
        let styled_total = style(&total_str).bold().green();
        println!("{styled_total:>term_width$}");
    }

    Ok(())
}

pub async fn generate_portfolio_summary(
    portfolio: &Portfolio,
    symbol_provider: &(dyn PriceProvider + Send + Sync),
    isin_provider: &(dyn PriceProvider + Send + Sync),
    currency_provider: &(dyn CurrencyRateProvider + Send + Sync),
    portfolio_currency: &str,
    pb: ProgressBar,
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
        if let crate::config::Investment::FixedDeposit(fd) = investment {
            let mut investment_summary = InvestmentSummary {
                symbol: fd.name.clone(),
                units: None,
                current_price: None,
                current_value: Some(fd.value),
                converted_value: None,
                currency: fd.currency.clone(),
                weight_pct: None,
                error: None,
            };

            let fd_currency = fd.currency.as_deref().unwrap_or(portfolio_currency);
            investment_summary.currency = Some(fd_currency.to_string());

            if fd_currency == portfolio_currency {
                total_converted_value += fd.value;
                investment_summary.converted_value = Some(fd.value);
            } else {
                match currency_provider
                    .get_rate(fd_currency, portfolio_currency)
                    .await
                {
                    Ok(rate) => {
                        let converted_value = fd.value * rate;
                        total_converted_value += converted_value;
                        investment_summary.converted_value = Some(converted_value);
                    }
                    Err(e) => {
                        investment_summary.error = Some(format!(
                            "Currency conversion failed from {fd_currency} to {portfolio_currency}: {e}",
                        ));
                        all_valid = false;
                    }
                }
            }
            summary.investments.push(investment_summary);
            pb.inc(1); // Increment the passed-in progress bar
            continue;
        }

        let (identifier, units, provider) = match investment {
            crate::config::Investment::Stock(s) => (
                s.symbol.clone(),
                s.units,
                symbol_provider as &(dyn PriceProvider + Send + Sync),
            ),
            crate::config::Investment::MutualFund(mf) => (
                mf.isin.clone(),
                mf.units,
                isin_provider as &(dyn PriceProvider + Send + Sync),
            ),
            _ => unreachable!(),
        };

        let price_result = provider.fetch_price(&identifier).await;

        let mut investment_summary = InvestmentSummary {
            symbol: identifier.clone(),
            units: Some(units),
            current_price: None,
            current_value: None,
            converted_value: None,
            currency: None,
            weight_pct: None,
            error: None,
        };

        match price_result {
            Ok(price_data) => {
                let value = units * price_data.price;
                investment_summary.current_price = Some(price_data.price);
                investment_summary.current_value = Some(value);
                investment_summary.currency = Some(price_data.currency.clone());

                // Perform currency conversion if needed
                if price_data.currency == portfolio_currency {
                    debug!(
                        "No currency conversion needed for {} ({} -> {})",
                        identifier, price_data.currency, portfolio_currency
                    );
                    total_converted_value += value;
                    investment_summary.converted_value = Some(value);
                } else {
                    debug!(
                        "Attempting currency conversion for {} ({} -> {})",
                        identifier, price_data.currency, portfolio_currency
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
                            debug!("Currency conversion error for {}: {}", identifier, e);
                        }
                    }
                }

                portfolio_value += value;
            }
            Err(e) => {
                all_valid = false;
                investment_summary.error = Some(e.to_string());
                debug!("Price fetch error for {}: {}", identifier, e);
            }
        };
        summary.investments.push(investment_summary);
        pb.inc(1); // Increment the passed-in progress bar
    }

    if all_valid {
        // Removed && portfolio_value > 0.0 condition here as total_converted_value is now key
        summary.converted_value = Some(total_converted_value);
        summary.total_value = Some(portfolio_value); // Keep original total_value
        for investment in &mut summary.investments {
            if let Some(value) = investment.converted_value {
                // Weight percentage is now based on converted_value
                if total_converted_value > 0.0 {
                    investment.weight_pct = Some((value / total_converted_value) * 100.0);
                }
            }
        }
    } else {
        // If not all valid, ensure converted_value and total_value are None
        summary.converted_value = None;
        summary.total_value = None; // Reset original total_value too if any error
    }

    pb.finish_and_clear();

    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FixedDepositInvestment, Investment, StockInvestment};
    use crate::currency_provider::CurrencyRateProvider;
    use crate::price_provider::{PriceProvider, PriceResult};
    use anyhow::{Result, anyhow};
    use async_trait::async_trait;
    use std::collections::HashMap;

    struct MockPriceProvider {
        prices: HashMap<String, PriceResult>,
        errors: HashMap<String, String>,
    }

    impl MockPriceProvider {
        fn new() -> Self {
            MockPriceProvider {
                prices: HashMap::new(),
                errors: HashMap::new(),
            }
        }

        fn add_price(&mut self, symbol: &str, price: PriceResult) {
            self.prices.insert(symbol.to_string(), price);
        }

        fn add_error(&mut self, symbol: &str, error_msg: &str) {
            self.errors
                .insert(symbol.to_string(), error_msg.to_string());
        }
    }

    #[async_trait]
    impl PriceProvider for MockPriceProvider {
        async fn fetch_price(&self, symbol: &str) -> Result<PriceResult> {
            if let Some(error_msg) = self.errors.get(symbol) {
                return Err(anyhow!(error_msg.clone()));
            }
            self.prices
                .get(symbol)
                .cloned()
                .ok_or_else(|| anyhow!("Price not found for {}", symbol))
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

    #[tokio::test]
    async fn test_valid_single_investment() {
        let currency_provider = MockCurrencyProvider::new();
        let mut symbol_provider = MockPriceProvider::new();
        symbol_provider.add_price(
            "AAPL",
            PriceResult {
                price: 150.0,
                currency: "USD".to_string(),
                historical: HashMap::new(),
            },
        );
        let isin_provider = MockPriceProvider::new();

        let portfolio = Portfolio {
            name: "Tech".to_string(),
            investments: vec![Investment::Stock(StockInvestment {
                symbol: "AAPL".to_string(),
                units: 10.0,
            })],
        };
        let summary = generate_portfolio_summary(
            &portfolio,
            &symbol_provider,
            &isin_provider,
            &currency_provider,
            "USD",
            ui::new_progress_bar(portfolio.investments.len() as u64, false),
        )
        .await;

        assert_eq!(summary.name, "Tech");
        assert_eq!(summary.total_value, Some(1500.0));
        assert_eq!(summary.converted_value, Some(1500.0));
        assert_eq!(summary.currency, Some("USD".to_string()));
        assert_eq!(summary.investments[0].symbol, "AAPL");
        assert_eq!(summary.investments[0].current_value, Some(1500.0));
        assert_eq!(summary.investments[0].converted_value, Some(1500.0));
        assert_eq!(summary.investments[0].weight_pct, Some(100.0));
        assert_eq!(summary.investments[0].error, None);
    }

    #[tokio::test]
    async fn test_error_handling_price_fetch() {
        let currency_provider = MockCurrencyProvider::new();
        let mut symbol_provider = MockPriceProvider::new();
        symbol_provider.add_price(
            "AAPL",
            PriceResult {
                price: 150.0,
                currency: "USD".to_string(),
                historical: HashMap::new(),
            },
        );
        symbol_provider.add_error("MSFT", "API unavailable");
        let isin_provider = MockPriceProvider::new();

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

        let summary = generate_portfolio_summary(
            &portfolio,
            &symbol_provider,
            &isin_provider,
            &currency_provider,
            "USD",
            ui::new_progress_bar(portfolio.investments.len() as u64, false),
        )
        .await;

        assert!(summary.converted_value.is_none());
        assert_eq!(summary.investments[0].error, None);
        assert_eq!(
            summary.investments[1].error.as_deref(),
            Some("API unavailable")
        );
        assert!(summary.investments[0].converted_value.is_some());
        assert!(summary.investments[1].converted_value.is_none());
    }

    #[tokio::test]
    async fn test_mixed_currencies_with_conversion() {
        let mut symbol_provider = MockPriceProvider::new();
        symbol_provider.add_price(
            "AAPL",
            PriceResult {
                price: 150.0,
                currency: "USD".to_string(),
                historical: HashMap::new(),
            },
        );
        symbol_provider.add_price(
            "RY",
            PriceResult {
                price: 100.0,
                currency: "CAD".to_string(),
                historical: HashMap::new(),
            },
        );
        let isin_provider = MockPriceProvider::new();
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

        let summary = generate_portfolio_summary(
            &portfolio,
            &symbol_provider,
            &isin_provider,
            &currency_provider,
            "USD",
            ui::new_progress_bar(portfolio.investments.len() as u64, false),
        )
        .await;

        assert_eq!(summary.name, "Diversified");
        assert_eq!(summary.converted_value, Some(2250.0));
        assert_eq!(summary.currency, Some("USD".to_string()));
        assert_eq!(summary.investments[0].symbol, "AAPL");
        assert_eq!(summary.investments[0].current_value, Some(1500.0));
        assert_eq!(summary.investments[0].converted_value, Some(1500.0));
        assert_eq!(
            summary.investments[0].weight_pct,
            Some((1500.0 / 2250.0) * 100.0)
        );
        assert_eq!(summary.investments[1].symbol, "RY");
        assert_eq!(summary.investments[1].current_value, Some(1000.0));
        assert_eq!(summary.investments[1].converted_value, Some(750.0));
        assert_eq!(
            summary.investments[1].weight_pct,
            Some((750.0 / 2250.0) * 100.0)
        );
    }

    #[tokio::test]
    async fn test_currency_conversion_error() {
        let mut symbol_provider = MockPriceProvider::new();
        symbol_provider.add_price(
            "AAPL",
            PriceResult {
                price: 150.0,
                currency: "USD".to_string(),
                historical: HashMap::new(),
            },
        );
        symbol_provider.add_price(
            "RY",
            PriceResult {
                price: 100.0,
                currency: "CAD".to_string(),
                historical: HashMap::new(),
            },
        );
        let isin_provider = MockPriceProvider::new();
        let mut currency_provider = MockCurrencyProvider::new();
        currency_provider.add_error("CAD", "USD", "Rate service unavailable");
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

        let summary = generate_portfolio_summary(
            &portfolio,
            &symbol_provider,
            &isin_provider,
            &currency_provider,
            "USD",
            ui::new_progress_bar(portfolio.investments.len() as u64, false),
        )
        .await;

        assert!(summary.converted_value.is_none());
        assert_eq!(summary.investments[0].error, None);
        assert_eq!(
            summary.investments[1].error,
            Some(
                "Currency conversion failed from CAD to USD: Rate service unavailable".to_string()
            )
        );
        assert!(summary.investments[0].converted_value.is_some());
        assert!(summary.investments[1].converted_value.is_none());
    }

    #[tokio::test]
    async fn test_fixed_deposit_investment() {
        let symbol_provider = MockPriceProvider::new();
        let isin_provider = MockPriceProvider::new();
        let currency_provider = MockCurrencyProvider::new();

        // Test with fixed deposit that specifies a currency
        let portfolio_with_currency = Portfolio {
            name: "Bank".to_string(),
            investments: vec![Investment::FixedDeposit(FixedDepositInvestment {
                name: "My FD".to_string(),
                value: 5000.0,
                currency: Some("INR".to_string()),
            })],
        };

        // Test with fixed deposit that does not specify a currency
        let portfolio_without_currency = Portfolio {
            name: "Bank".to_string(),
            investments: vec![Investment::FixedDeposit(FixedDepositInvestment {
                name: "My FD".to_string(),
                value: 6000.0,
                currency: None,
            })],
        };

        // Test portfolio with specified currency
        let summary_with_currency = generate_portfolio_summary(
            &portfolio_with_currency,
            &symbol_provider,
            &isin_provider,
            &currency_provider,
            "INR",
            ui::new_progress_bar(portfolio.investments.len() as u64, false),
        )
        .await;

        assert_eq!(summary_with_currency.name, "Bank");
        assert_eq!(summary_with_currency.converted_value, Some(5000.0));
        assert_eq!(summary_with_currency.investments.len(), 1);
        assert_eq!(summary_with_currency.investments[0].symbol, "My FD");
        assert_eq!(summary_with_currency.investments[0].units, None);
        assert_eq!(
            summary_with_currency.investments[0].converted_value,
            Some(5000.0)
        );
        assert_eq!(
            summary_with_currency.investments[0].currency,
            Some("INR".to_string())
        );
        assert_eq!(summary_with_currency.investments[0].weight_pct, Some(100.0));

        // Test portfolio without specified currency
        let summary_without_currency = generate_portfolio_summary(
            &portfolio_without_currency,
            &symbol_provider,
            &isin_provider,
            &currency_provider,
            "INR",
        )
        .await;

        assert_eq!(summary_without_currency.name, "Bank");
        assert_eq!(summary_without_currency.converted_value, Some(6000.0));
        assert_eq!(summary_without_currency.investments.len(), 1);
        assert_eq!(summary_without_currency.investments[0].symbol, "My FD");
        assert_eq!(summary_without_currency.investments[0].units, None);
        assert_eq!(
            summary_without_currency.investments[0].converted_value,
            Some(6000.0)
        );
        assert_eq!(
            summary_without_currency.investments[0].currency,
            Some("INR".to_string())
        );
        assert_eq!(
            summary_without_currency.investments[0].weight_pct,
            Some(100.0)
        );

        // Test with non-matching currency (should trigger conversion)
        let mut currency_provider_with_rate = MockCurrencyProvider::new();
        currency_provider_with_rate.add_rate("USD", "INR", 80.0);
        let portfolio_usd_fd = Portfolio {
            name: "Bank".to_string(),
            investments: vec![Investment::FixedDeposit(FixedDepositInvestment {
                name: "USD FD".to_string(),
                value: 100.0,
                currency: Some("USD".to_string()),
            })],
        };

        let summary_usd = generate_portfolio_summary(
            &portfolio_usd_fd,
            &symbol_provider,
            &isin_provider,
            &currency_provider_with_rate,
            "INR",
        )
        .await;

        assert_eq!(summary_usd.name, "Bank");
        assert_eq!(summary_usd.converted_value, Some(8000.0));
        assert_eq!(summary_usd.investments.len(), 1);
        assert_eq!(summary_usd.investments[0].symbol, "USD FD");
        assert_eq!(summary_usd.investments[0].units, None);
        assert_eq!(summary_usd.investments[0].converted_value, Some(8000.0));
        assert_eq!(summary_usd.investments[0].currency, Some("USD".to_string()));
        assert_eq!(summary_usd.investments[0].weight_pct, Some(100.0));
        assert_eq!(summary_usd.investments[0].error, None);

        // Test with currency conversion error
        let mut currency_provider_with_error = MockCurrencyProvider::new();
        currency_provider_with_error.add_error("USD", "INR", "Rate unavailable");
        let summary_error = generate_portfolio_summary(
            &portfolio_usd_fd,
            &symbol_provider,
            &isin_provider,
            &currency_provider_with_error,
            "INR",
        )
        .await;

        assert!(summary_error.converted_value.is_none());
        assert_eq!(
            summary_error.investments[0].error,
            Some("Currency conversion failed from USD to INR: Rate unavailable".to_string())
        );
    }

    #[tokio::test]
    async fn test_mixed_investments_with_fixed_deposit() {
        let mut symbol_provider = MockPriceProvider::new();
        symbol_provider.add_price(
            "AAPL",
            PriceResult {
                price: 200.0,
                currency: "USD".to_string(),
                historical: HashMap::new(),
            },
        );
        let isin_provider = MockPriceProvider::new();
        let mut currency_provider = MockCurrencyProvider::new();
        currency_provider.add_rate("USD", "INR", 80.0);

        let portfolio = Portfolio {
            name: "Mixed Portfolio".to_string(),
            investments: vec![
                Investment::Stock(StockInvestment {
                    symbol: "AAPL".to_string(),
                    units: 10.0, // 2000 USD
                }),
                Investment::FixedDeposit(FixedDepositInvestment {
                    name: "My FD".to_string(),
                    value: 40000.0, // 40000 INR
                    currency: Some("INR".to_string()),
                }),
            ],
        };

        let summary = generate_portfolio_summary(
            &portfolio,
            &symbol_provider,
            &isin_provider,
            &currency_provider,
            "INR",
        )
        .await;

        // AAPL value in INR = 10 * 200 * 80 = 160000 INR
        // FD value in INR = 40000 INR
        // Total = 200000 INR
        assert_eq!(summary.converted_value, Some(200000.0));
        let aapl_summary = summary
            .investments
            .iter()
            .find(|i| i.symbol == "AAPL")
            .unwrap();
        let fd_summary = summary
            .investments
            .iter()
            .find(|i| i.symbol == "My FD")
            .unwrap();

        assert_eq!(aapl_summary.converted_value, Some(160000.0));
        assert_eq!(fd_summary.converted_value, Some(40000.0));

        assert_eq!(aapl_summary.weight_pct, Some(80.0));
        assert_eq!(fd_summary.weight_pct, Some(20.0));
    }
}
