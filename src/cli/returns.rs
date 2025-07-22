use super::ui;
use crate::core::{
    analytics, CurrencyRateProvider, HistoricalPeriod, PriceProvider, PriceResult,
    config::{Investment, Portfolio},
};
use anyhow::{Result, anyhow};
use comfy_table::{Attribute, Cell};
use futures::future::join_all;
use rust_decimal::{Decimal, prelude::*};
use rust_finprim::rate::cagr;
use std::collections::{BTreeMap, HashMap};
use tracing::{debug, info};

#[derive(Clone)]
struct ReturnResult {
    identifier: String,
    short_name: Option<String>,
    cagrs: BTreeMap<HistoricalPeriod, f64>,
    error: Option<String>,
}

struct PortfolioReturnResult {
    name: String,
    investment_returns: Vec<ReturnResult>,
    portfolio_cagrs: BTreeMap<HistoricalPeriod, f64>,
}

pub async fn run(
    portfolios: &[Portfolio],
    symbol_provider: &(dyn PriceProvider + Send + Sync),
    isin_provider: &(dyn PriceProvider + Send + Sync),
    currency_provider: &(dyn CurrencyRateProvider + Send + Sync),
    target_currency: &str,
) -> anyhow::Result<()> {
    info!("Calculating returns for investments...");

    let mut investments_to_fetch = HashMap::new();
    for portfolio in portfolios {
        for investment in &portfolio.investments {
            match investment {
                Investment::Stock(s) => {
                    investments_to_fetch.insert(s.symbol.clone(), symbol_provider);
                }
                Investment::MutualFund(mf) => {
                    investments_to_fetch.insert(mf.isin.clone(), isin_provider);
                }
                Investment::FixedDeposit(_) => {} // Not relevant for returns
            }
        }
    }

    if investments_to_fetch.is_empty() {
        println!("No investments found to calculate returns for.");
        return Ok(());
    }

    // Step 1: Fetch all prices concurrently
    let pb = ui::new_progress_bar(investments_to_fetch.len() as u64, false);

    let futures = investments_to_fetch.into_iter().map(|(id, provider)| {
        let pb_clone = pb.clone();
        async move {
            let result = provider.fetch_price(&id).await;
            pb_clone.inc(1);
            (id, result)
        }
    });

    let fetched_results: HashMap<String, Result<PriceResult>> =
        join_all(futures).await.into_iter().collect();
    pb.finish_and_clear();

    // Step 2: Process results for each portfolio
    let num_portfolios = portfolios.len();
    for (i, portfolio) in portfolios.iter().enumerate() {
        let result = calculate_portfolio_returns(
            portfolio,
            &fetched_results,
            currency_provider,
            target_currency,
        )
        .await;

        if !result.investment_returns.is_empty() {
            println!(
                "\nPortfolio: {}",
                ui::style_text(&result.name, ui::StyleType::Title)
            );
            display_return_results(&result);

            if i < num_portfolios - 1 {
                ui::print_separator();
            }
        }
    }

    Ok(())
}

async fn calculate_portfolio_returns(
    portfolio: &Portfolio,
    price_results: &HashMap<String, Result<PriceResult>>,
    currency_provider: &(dyn CurrencyRateProvider + Send + Sync),
    target_currency: &str,
) -> PortfolioReturnResult {
    let holdings = analytics::calculate_portfolio_holdings(
        portfolio,
        price_results,
        currency_provider,
        target_currency,
        &|| (), // No progress updates needed here
    )
    .await;

    let mut investment_returns = Vec::new();
    let mut portfolio_cagrs: BTreeMap<HistoricalPeriod, f64> = BTreeMap::new();
    let mut period_contributors: BTreeMap<HistoricalPeriod, f64> = BTreeMap::new();

    for holding in &holdings.investments {
        if holding.units.is_none() {
            continue;
        }

        if let Some(e) = &holding.error {
            investment_returns.push(ReturnResult {
                identifier: holding.identifier.clone(),
                short_name: holding.short_name.clone(),
                cagrs: BTreeMap::new(),
                error: Some(e.clone()),
            });
            continue;
        }

        let mut result = ReturnResult {
            identifier: holding.identifier.clone(),
            short_name: holding.short_name.clone(),
            cagrs: BTreeMap::new(),
            error: None,
        };

        if let Some(Ok(price_data)) = price_results.get(&holding.identifier) {
            match calculate_cagr(price_data) {
                Ok(cagrs) => {
                    if let Some(weight) = holding.weight {
                        for (period, cagr_val) in &cagrs {
                            let weighted_value = cagr_val * (weight / 100.0);
                            *portfolio_cagrs.entry(*period).or_insert(0.0) += weighted_value;
                            *period_contributors.entry(*period).or_insert(0.0) += weight / 100.0;
                        }
                    }
                    result.cagrs = cagrs;
                }
                Err(e) => {
                    result.error = Some(format!("CAGR calculation failed: {e}"));
                }
            }
        } else {
            result.error = Some("Price data not available".to_string());
        }

        investment_returns.push(result);
    }

    for (period, total_weight) in &period_contributors {
        if let Some(weighted_cagr) = portfolio_cagrs.get_mut(period) {
            if *total_weight > 0.0 {
                *weighted_cagr /= *total_weight;
            }
        }
    }

    PortfolioReturnResult {
        name: portfolio.name.clone(),
        investment_returns,
        portfolio_cagrs,
    }
}

fn calculate_cagr(price_data: &PriceResult) -> Result<BTreeMap<HistoricalPeriod, f64>> {
    let mut cagrs = BTreeMap::new();
    let periods = [
        HistoricalPeriod::OneYear,
        HistoricalPeriod::ThreeYears,
        HistoricalPeriod::FiveYears,
        HistoricalPeriod::TenYears,
    ];

    for &period in &periods {
        if let Some(historical_price) = price_data.historical_prices.get(&period) {
            if *historical_price <= 0.0 || price_data.price <= 0.0 {
                continue;
            }

            let duration_days = period.to_duration().num_days() as f64;
            let duration_years = duration_days / 365.0;

            if duration_years <= 0.0 {
                continue;
            }

            debug!(
                "historical price: {:?}, {duration_years}yrs",
                *historical_price
            );
            let begin_bal = Decimal::from_f64(*historical_price)
                .ok_or_else(|| anyhow!("Invalid historical price"))?;
            let end_bal = Decimal::from_f64(price_data.price)
                .ok_or_else(|| anyhow!("Invalid current price"))?;
            let n_years =
                Decimal::from_f64(duration_years).ok_or_else(|| anyhow!("Invalid duration"))?;

            if n_years.is_zero() {
                continue;
            }

            let rate = cagr(begin_bal, end_bal, n_years);
            let percentage = (rate * Decimal::from(100))
                .to_f64()
                .ok_or_else(|| anyhow!("CAGR percentage conversion failed"))?;
            cagrs.insert(period, percentage);

            debug!("cagr: {begin_bal}, {end_bal}, {n_years} = {rate}, {percentage}");
        }
    }

    if cagrs.is_empty() {
        Err(anyhow!(
            "No historical prices available for CAGR calculation"
        ))
    } else {
        Ok(cagrs)
    }
}

fn display_return_results(result: &PortfolioReturnResult) {
    let mut table = ui::new_styled_table();
    let periods = [
        HistoricalPeriod::OneYear,
        HistoricalPeriod::ThreeYears,
        HistoricalPeriod::FiveYears,
        HistoricalPeriod::TenYears,
    ];

    let mut header = vec![ui::header_cell("Investment")];
    for period in &periods {
        header.push(ui::header_cell(&period.to_string()));
    }
    table.set_header(header);

    for result in &result.investment_returns {
        let name_display = if let Some(name) = &result.short_name {
            name.clone()
        } else {
            result.identifier.clone()
        };
        let mut row_cells = vec![Cell::new(name_display)];

        for period in &periods {
            let cell = match result.cagrs.get(period) {
                Some(cagr) => ui::change_cell(*cagr),
                None => ui::na_cell(result.error.is_some()),
            };
            row_cells.push(cell);
        }
        table.add_row(row_cells);
    }

    if !result.portfolio_cagrs.is_empty() && result.investment_returns.len() > 1 {
        let mut total_row_cells =
            vec![Cell::new("Portfolio Weighted").add_attribute(Attribute::Bold)];
        for period in &periods {
            let cell = match result.portfolio_cagrs.get(period) {
                Some(cagr) => ui::change_cell(*cagr),
                None => ui::na_cell(false),
            };
            total_row_cells.push(cell);
        }
        table.add_row(total_row_cells);
    }

    println!("{table}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::StockInvestment;
    use crate::core::currency::CurrencyRateProvider;
    use anyhow::Result;
    use async_trait::async_trait;
    use crate::core::price::{HistoricalPeriod, PriceResult};
    use std::collections::HashMap;

    fn create_test_data() -> PriceResult {
        PriceResult {
            price: 100.0,
            currency: "USD".to_string(),
            historical_prices: HashMap::from([
                (HistoricalPeriod::OneYear, 80.0),
                (HistoricalPeriod::ThreeYears, 50.0),
            ]),
            short_name: Some("TEST".to_string()),
        }
    }

    #[test]
    fn calculates_cagr_for_all_periods() {
        let data = create_test_data();
        let cagrs = calculate_cagr(&data).unwrap();

        assert_eq!(cagrs.len(), 2);
        assert!((cagrs[&HistoricalPeriod::OneYear] - 25.0).abs() < 0.1);
        assert!((cagrs[&HistoricalPeriod::ThreeYears] - 25.99).abs() < 0.1);
    }

    #[test]
    fn handles_missing_historical_data() {
        let data = PriceResult {
            price: 100.0,
            currency: "USD".to_string(),
            historical_prices: HashMap::new(),
            short_name: None,
        };

        assert!(calculate_cagr(&data).is_err());
    }

    // A mock currency provider that assumes all currencies are 1:1 with target
    struct MockCurrencyProvider;

    #[async_trait]
    impl CurrencyRateProvider for MockCurrencyProvider {
        async fn get_rate(&self, from: &str, to: &str) -> Result<f64> {
            if from == to {
                Ok(1.0)
            } else {
                Ok(1.0) // Assume 1:1 for simplicity in tests
            }
        }
    }

    #[tokio::test]
    async fn test_calculate_portfolio_returns_weighted() {
        let portfolio = Portfolio {
            name: "Tech".to_string(),
            investments: vec![
                Investment::Stock(StockInvestment {
                    symbol: "AAPL".to_string(),
                    units: 10.0, // value 1000
                }),
                Investment::Stock(StockInvestment {
                    symbol: "GOOG".to_string(),
                    units: 20.0, // value 1000
                }),
            ],
        };

        let mut price_results = HashMap::new();
        price_results.insert(
            "AAPL".to_string(),
            Ok(PriceResult {
                price: 100.0,
                currency: "USD".to_string(),
                short_name: Some("Apple".to_string()),
                historical_prices: HashMap::from([(HistoricalPeriod::OneYear, 80.0)]), // +25%
            }),
        );
        price_results.insert(
            "GOOG".to_string(),
            Ok(PriceResult {
                price: 50.0,
                currency: "USD".to_string(),
                short_name: Some("Google".to_string()),
                historical_prices: HashMap::from([(HistoricalPeriod::OneYear, 40.0)]), // +25%
            }),
        );

        let currency_provider = MockCurrencyProvider;
        let result =
            calculate_portfolio_returns(&portfolio, &price_results, &currency_provider, "USD")
                .await;

        // Each stock has 50% weight. (10*100 = 1000, 20*50 = 1000)
        // Both have 25% CAGR. Weighted average should be 25%.
        assert!((result.portfolio_cagrs[&HistoricalPeriod::OneYear] - 25.0).abs() < 0.1);
    }

    #[tokio::test]
    async fn test_calculate_portfolio_returns_with_missing_period() {
        let portfolio = Portfolio {
            name: "Tech".to_string(),
            investments: vec![
                Investment::Stock(StockInvestment {
                    symbol: "AAPL".to_string(),
                    units: 10.0, // value 1000 (50% weight)
                }),
                Investment::Stock(StockInvestment {
                    symbol: "GOOG".to_string(),
                    units: 20.0, // value 1000 (50% weight)
                }),
            ],
        };
        let mut price_results = HashMap::new();
        price_results.insert(
            "AAPL".to_string(),
            Ok(PriceResult {
                price: 100.0,
                currency: "USD".to_string(),
                short_name: Some("Apple".to_string()),
                historical_prices: HashMap::from([(HistoricalPeriod::OneYear, 80.0)]), // +25%
            }),
        );
        price_results.insert(
            "GOOG".to_string(),
            Ok(PriceResult {
                price: 50.0,
                currency: "USD".to_string(),
                short_name: Some("Google".to_string()),
                historical_prices: HashMap::new(),
            }),
        );
        let currency_provider = MockCurrencyProvider;
        let result =
            calculate_portfolio_returns(&portfolio, &price_results, &currency_provider, "USD")
                .await;
        assert!((result.portfolio_cagrs[&HistoricalPeriod::OneYear] - 25.0).abs() < 0.1);
    }
}
