use super::ui;
use crate::core::config::{Investment, Portfolio};
use crate::core::{analytics, CurrencyRateProvider, HistoricalPeriod, PriceProvider, PriceResult};
use anyhow::Result;
use comfy_table::{Attribute, Cell};
use futures::future::join_all;
use std::collections::BTreeMap;
use std::collections::HashMap;

#[derive(Clone)]
struct ChangeResult {
    identifier: String,
    short_name: Option<String>,
    changes: BTreeMap<HistoricalPeriod, f64>,
    error: Option<String>,
}

struct PortfolioChangeResult {
    name: String,
    investment_changes: Vec<ChangeResult>,
    portfolio_changes: BTreeMap<HistoricalPeriod, f64>,
}

pub async fn run(
    portfolios: &[Portfolio],
    symbol_provider: &(dyn PriceProvider + Send + Sync),
    isin_provider: &(dyn PriceProvider + Send + Sync),
    currency_provider: &(dyn CurrencyRateProvider + Send + Sync),
    target_currency: &str,
) -> anyhow::Result<()> {
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
                Investment::FixedDeposit(_) => {}
            }
        }
    }

    if investments_to_fetch.is_empty() {
        println!("No stock or mutual fund investments found to display changes for.");
        return Ok(());
    }

    // Step 1: Fetch all prices concurrently
    let pb = ui::new_progress_bar(investments_to_fetch.len() as u64, false);
    let price_futures = investments_to_fetch.iter().map(|(id, provider)| {
        let pb_clone = pb.clone();
        async move {
            let res = provider.fetch_price(id).await;
            pb_clone.inc(1);
            (id.clone(), res)
        }
    });
    let price_results: HashMap<String, Result<PriceResult>> =
        join_all(price_futures).await.into_iter().collect();
    pb.finish_and_clear();

    // Step 2: Process results for each portfolio
    let num_portfolios = portfolios.len();
    for (i, portfolio) in portfolios.iter().enumerate() {
        let result = calculate_portfolio_changes(
            portfolio,
            &price_results,
            currency_provider,
            target_currency,
        )
        .await;

        if !result.investment_changes.is_empty() {
            println!(
                "\nPortfolio: {}",
                ui::style_text(&result.name, ui::StyleType::Title)
            );
            display_results(&result);

            if i < num_portfolios - 1 {
                ui::print_separator();
            }
        }
    }

    Ok(())
}

async fn calculate_portfolio_changes(
    portfolio: &Portfolio,
    price_results: &HashMap<String, Result<PriceResult>>,
    currency_provider: &(dyn CurrencyRateProvider + Send + Sync),
    target_currency: &str,
) -> PortfolioChangeResult {
    // First, get weights for all investments in the portfolio
    let holdings = analytics::calculate_portfolio_value(
        portfolio,
        price_results,
        currency_provider,
        target_currency,
        &|| (), // No progress updates needed here
    )
    .await;

    let mut investment_changes = Vec::new();
    let mut portfolio_changes: BTreeMap<HistoricalPeriod, f64> = BTreeMap::new();
    let mut period_contributors: BTreeMap<HistoricalPeriod, f64> = BTreeMap::new();

    for holding in &holdings.investments {
        // Skip fixed deposits as they don't have historical price changes
        if holding.units.is_none() {
            continue;
        }

        if let Some(e) = &holding.error {
            investment_changes.push(ChangeResult {
                identifier: holding.identifier.clone(),
                short_name: holding.short_name.clone(),
                changes: BTreeMap::new(),
                error: Some(e.clone()),
            });
            continue;
        }

        // Calculate percentage change for this investment
        let changes = if let Some(Ok(price_data)) = price_results.get(&holding.identifier) {
            price_data
                .historical_prices
                .iter()
                .filter_map(|(period, historical_price)| {
                    if *historical_price > 0.0 {
                        let change =
                            ((price_data.price - historical_price) / historical_price) * 100.0;
                        Some((*period, change))
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            BTreeMap::new()
        };

        // Add this investment's weighted change to the portfolio total
        if let Some(weight) = holding.weight {
            for (period, change) in &changes {
                let weighted_value = change * (weight / 100.0);
                *portfolio_changes.entry(*period).or_insert(0.0) += weighted_value;
                *period_contributors.entry(*period).or_insert(0.0) += weight / 100.0;
            }
        }

        investment_changes.push(ChangeResult {
            identifier: holding.identifier.clone(),
            short_name: holding.short_name.clone(),
            changes,
            error: None,
        });
    }

    // Normalize weighted changes for periods where total weight might not be 100%
    for (period, total_weight) in &period_contributors {
        if let Some(weighted_change) = portfolio_changes.get_mut(period) {
            if *total_weight > 0.0 {
                *weighted_change /= *total_weight;
            }
        }
    }

    PortfolioChangeResult {
        name: portfolio.name.clone(),
        investment_changes,
        portfolio_changes,
    }
}

fn display_results(result: &PortfolioChangeResult) {
    let mut table = ui::new_styled_table();

    let mut periods: Vec<HistoricalPeriod> = vec![
        HistoricalPeriod::OneDay,
        HistoricalPeriod::FiveDays,
        HistoricalPeriod::OneMonth,
        HistoricalPeriod::OneYear,
        HistoricalPeriod::ThreeYears,
        HistoricalPeriod::FiveYears,
        HistoricalPeriod::TenYears,
    ];
    periods.sort(); // Ensure consistent order

    let mut header = vec![ui::header_cell("Identifier")];
    for period in &periods {
        header.push(ui::header_cell(&period.to_string()));
    }
    table.set_header(header);

    for result in &result.investment_changes {
        let identifier_cell_content = if let Some(name) = &result.short_name {
            name.clone()
        } else {
            result.identifier.clone()
        };
        let mut row_cells = vec![Cell::new(identifier_cell_content)];

        for period in &periods {
            let cell = match result.changes.get(period) {
                Some(change) => ui::change_cell(*change),
                None => ui::na_cell(result.error.is_some()),
            };
            row_cells.push(cell);
        }
        table.add_row(row_cells);
    }

    if !result.portfolio_changes.is_empty() && result.investment_changes.len() > 1 {
        let mut total_row_cells =
            vec![Cell::new("Portfolio Weighted").add_attribute(Attribute::Bold)];
        for period in &periods {
            let cell = match result.portfolio_changes.get(period) {
                Some(change) => ui::change_cell(*change),
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
    use crate::core::config::{StockInvestment, Investment};
    use crate::core::currency::CurrencyRateProvider;
    use anyhow::Result;
    use async_trait::async_trait;

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
    async fn test_calculate_portfolio_changes_weighted() {
        let portfolio = Portfolio {
            name: "Tech".to_string(),
            investments: vec![
                Investment::Stock(StockInvestment {
                    symbol: "AAPL".to_string(),
                    units: 10.0, // value 1000
                }),
                Investment::Stock(StockInvestment {
                    symbol: "GOOG".to_string(),
                    units: 5.0, // value 1000
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
                historical_prices: HashMap::from([(HistoricalPeriod::OneDay, 90.0)]), // +11.11%
            }),
        );
        price_results.insert(
            "GOOG".to_string(),
            Ok(PriceResult {
                price: 200.0,
                currency: "USD".to_string(),
                short_name: Some("Google".to_string()),
                historical_prices: HashMap::from([(HistoricalPeriod::OneDay, 180.0)]), // +11.11%
            }),
        );

        let currency_provider = MockCurrencyProvider;
        let result = calculate_portfolio_changes(
            &portfolio,
            &price_results,
            &currency_provider,
            "USD",
        )
        .await;

        assert_eq!(result.name, "Tech");
        assert_eq!(result.investment_changes.len(), 2);
        assert_eq!(result.portfolio_changes.len(), 1);

        // Each stock is worth 1000, so 50% weight each.
        // Weighted average is just the average of the two.
        // (11.111... + 11.111...) / 2 = 11.111...
        let aapl_change = result.investment_changes[0].changes[&HistoricalPeriod::OneDay];
        assert!((aapl_change - 11.11).abs() < 0.02);

        let weighted_change = result.portfolio_changes[&HistoricalPeriod::OneDay];
        assert!((weighted_change - 11.11).abs() < 0.02);
    }

    #[tokio::test]
    async fn test_calculate_portfolio_changes_with_uneven_weights() {
        let portfolio = Portfolio {
            name: "Tech".to_string(),
            investments: vec![
                Investment::Stock(StockInvestment {
                    symbol: "AAPL".to_string(),
                    units: 15.0, // value 1500 (75% weight)
                }),
                Investment::Stock(StockInvestment {
                    symbol: "GOOG".to_string(),
                    units: 2.5, // value 500 (25% weight)
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
                historical_prices: HashMap::from([(HistoricalPeriod::OneDay, 90.0)]), // +11.11%
            }),
        );
        price_results.insert(
            "GOOG".to_string(),
            Ok(PriceResult {
                price: 200.0,
                currency: "USD".to_string(),
                short_name: Some("Google".to_string()),
                historical_prices: HashMap::from([(HistoricalPeriod::OneDay, 180.0)]), // +11.11%
            }),
        );

        let currency_provider = MockCurrencyProvider;
        let result = calculate_portfolio_changes(
            &portfolio,
            &price_results,
            &currency_provider,
            "USD",
        )
        .await;

        // Weighted average should still be the same since individual changes are the same
        let weighted_change = result.portfolio_changes[&HistoricalPeriod::OneDay];
        assert!((weighted_change - 11.11).abs() < 0.02);
    }

    #[tokio::test]
    async fn test_calculate_portfolio_changes_with_missing_period() {
        let portfolio = Portfolio {
            name: "Tech".to_string(),
            investments: vec![
                Investment::Stock(StockInvestment {
                    symbol: "AAPL".to_string(),
                    units: 10.0, // value 1000 (50% weight)
                }),
                Investment::Stock(StockInvestment {
                    symbol: "GOOG".to_string(),
                    units: 5.0, // value 1000 (50% weight)
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
                historical_prices: HashMap::from([
                    (HistoricalPeriod::OneDay, 90.0), // +11.11%
                    (HistoricalPeriod::FiveDays, 80.0), // +25%
                ]),
            }),
        );
        // GOOG is missing the FiveDays period
        price_results.insert(
            "GOOG".to_string(),
            Ok(PriceResult {
                price: 200.0,
                currency: "USD".to_string(),
                short_name: Some("Google".to_string()),
                historical_prices: HashMap::from([(HistoricalPeriod::OneDay, 180.0)]), // +11.11%
            }),
        );

        let currency_provider = MockCurrencyProvider;
        let result = calculate_portfolio_changes(
            &portfolio,
            &price_results,
            &currency_provider,
            "USD",
        )
        .await;

        let one_day_change = result.portfolio_changes[&HistoricalPeriod::OneDay];
        assert!((one_day_change - 11.11).abs() < 0.02, "1D change was {one_day_change}");

        // For 5D, only AAPL contributes. Its weight among contributors is 100%.
        // So the portfolio change for 5D should just be AAPL's change.
        let five_day_change = result.portfolio_changes[&HistoricalPeriod::FiveDays];
        assert!((five_day_change - 25.0).abs() < 0.01, "5D change was {five_day_change}");
    }
}
