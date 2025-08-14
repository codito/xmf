use super::ui;
use crate::core::analytics;
use crate::core::config::{Investment, Portfolio};
use crate::core::currency::CurrencyRateProvider;
use crate::core::metadata::MetadataProvider;
use crate::core::price::{PriceProvider, PriceResult};
use anyhow::Result;
use comfy_table::Cell;
use futures::future::join_all;
use std::collections::HashMap;

pub async fn run(
    portfolios: &[Portfolio],
    symbol_provider: &(dyn PriceProvider + Send + Sync),
    isin_provider: &(dyn PriceProvider + Send + Sync),
    currency_provider: &(dyn CurrencyRateProvider + Send + Sync),
    metadata_provider: &(dyn MetadataProvider + Send + Sync),
    target_currency: &str,
) -> Result<()> {
    // Pre-fetch prices for all investments across portfolios
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
                Investment::FixedDeposit(_) => {} // Skip price fetch for FDs
            }
        }
    }

    let pb = ui::new_progress_bar(investments_to_fetch.len() as u64, true);
    pb.set_message("Fetching prices...");

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

    let all_investments = portfolios
        .iter()
        .map(|p| p.investments.len())
        .sum::<usize>() as u64;
    let pb = ui::new_progress_bar(all_investments, true);
    pb.set_message("Calculating allocation...");

    // Cache metadata for mutual funds
    let mut metadata_cache: HashMap<String, String> = HashMap::new();
    let mut portfolio_values = Vec::new();

    for portfolio in portfolios {
        // Calculate portfolio value with conversions
        let portfolio_value = analytics::calculate_portfolio_value(
            portfolio,
            &price_results,
            currency_provider,
            target_currency,
            &|| pb.inc(1),
        )
        .await;
        portfolio_values.push(portfolio_value);
    }

    pb.finish_and_clear();

    // Display allocation for each portfolio
    for (i, portfolio_value) in portfolio_values.iter().enumerate() {
        // Skip empty portfolios
        if portfolio_value.investments.is_empty() {
            continue;
        }

        // Accumulate investments by category (using raw fund_category strings)
        let mut categories: HashMap<String, Vec<(&Investment, f64)>> = HashMap::new();
        let portfolio = &portfolios[i];

        for (investment, value) in portfolio
            .investments
            .iter()
            .zip(portfolio_value.investments.iter())
        {
            if let Some(v) = value.converted_value {
                let category = match investment {
                    Investment::Stock(_) => "Equity".to_string(),
                    Investment::FixedDeposit(_) => "Debt".to_string(),
                    Investment::MutualFund(mf) => {
                        if let Some(cat) = metadata_cache.get(&mf.isin) {
                            cat.clone()
                        } else {
                            let fetched_category =
                                match metadata_provider.fetch_metadata(&mf.isin).await {
                                    Ok(meta) => meta.fund_type.clone(),
                                    Err(_) => "Other".to_string(),
                                };
                            metadata_cache.insert(mf.isin.clone(), fetched_category.clone());
                            fetched_category
                        }
                    }
                };
                categories
                    .entry(category)
                    .or_default()
                    .push((investment, v));
            }
        }

        display_allocation_table(
            &portfolio.name,
            categories,
            portfolio_value.total_converted_value,
            target_currency,
            &price_results,
        );
    }

    Ok(())
}

fn display_allocation_table(
    portfolio_name: &str,
    allocation: HashMap<String, Vec<(&Investment, f64)>>,
    total_value: Option<f64>,
    target_currency: &str,
    price_results: &HashMap<String, Result<PriceResult>>,
) {
    let mut table = ui::new_styled_table();
    table.set_header(vec![
        ui::header_cell("Category"),
        ui::header_cell("Investment"),
        ui::header_cell("Value"),
        ui::header_cell("Allocation"),
    ]);

    // Calculate portfolio total
    let total = total_value.unwrap_or_else(|| {
        allocation
            .values()
            .flat_map(|investments| investments.iter().map(|(_, v)| *v))
            .sum()
    });

    // Convert to Vec for sorting
    let mut categories: Vec<_> = allocation.into_iter().collect();
    // Sort by total category value (descending)
    categories.sort_by(|(_, a), (_, b)| {
        let a_total: f64 = a.iter().map(|(_, v)| v).sum();
        let b_total: f64 = b.iter().map(|(_, v)| v).sum();
        b_total.partial_cmp(&a_total).unwrap()
    });

    for (category, investments) in &mut categories {
        // Within category, sort investments by value (descending)
        investments.sort_by(|(_, a), (_, b)| b.partial_cmp(a).unwrap());

        // Calculate category total
        let category_total: f64 = investments.iter().map(|(_, v)| v).sum();
        let category_percentage = if total > 0.0 {
            category_total / total * 100.0
        } else {
            0.0
        };

        // Display category row using raw category string
        table.add_row(vec![
            Cell::new(&category),
            Cell::new(""),
            Cell::new(format!("{:.2} {}", category_total, target_currency)),
            Cell::new(format!("{:.2}%", category_percentage)),
        ]);

        // Display investments in this category
        for (investment, value) in investments {
            let display_name = match investment {
                Investment::Stock(stock) => price_results
                    .get(&stock.symbol)
                    .and_then(|pr| pr.as_ref().ok())
                    .and_then(|pr| pr.short_name.clone())
                    .unwrap_or_else(|| stock.symbol.clone()),
                Investment::MutualFund(mf) => price_results
                    .get(&mf.isin)
                    .and_then(|pr| pr.as_ref().ok())
                    .and_then(|pr| pr.short_name.clone())
                    .unwrap_or_else(|| mf.isin.clone()),
                Investment::FixedDeposit(fd) => fd.name.clone(),
            };

            let allocation_perc = if total > 0.0 {
                *value / total * 100.0
            } else {
                0.0
            };

            table.add_row(vec![
                Cell::new(""),
                Cell::new(display_name),
                Cell::new(ui::style_text(
                    &format!("{:.2} {}", *value, target_currency),
                    ui::StyleType::Subtle,
                )),
                ui::format_percentage_cell(allocation_perc),
            ]);
        }
    }

    // Display portfolio header
    println!(
        "\nPortfolio: {}\n",
        ui::style_text(portfolio_name, ui::StyleType::Title)
    );

    // Display the table
    println!("{table}");

    // Print portfolio total after the table
    if let Some(total) = total_value {
        println!(
            "\nPortfolio Total Value ({}): {:.2}\n",
            target_currency, total
        );
    } else {
        println!("\nPortfolio Total Value: N/A\n");
    }

    ui::print_separator();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::{FixedDepositInvestment, MutualFundInvestment, StockInvestment};
    use crate::core::currency::CurrencyRateProvider;
    use crate::core::metadata::{FundMetadata, MetadataProvider};
    use crate::core::price::PriceResult;
    use std::collections::HashMap;

    // Define mock currency provider
    struct MockCurrencyProvider;

    #[async_trait::async_trait]
    impl CurrencyRateProvider for MockCurrencyProvider {
        async fn get_rate(&self, _from: &str, _to: &str) -> anyhow::Result<f64> {
            Ok(1.0)
        }
    }

    // Create a mock metadata provider for testing
    struct MockMetadataProviderImpl;

    #[async_trait::async_trait]
    impl MetadataProvider for MockMetadataProviderImpl {
        async fn fetch_metadata(&self, identifier: &str) -> anyhow::Result<FundMetadata> {
            use chrono::NaiveDate;
            match identifier {
                "EQUITY_FUND" => Ok(FundMetadata {
                    isin: "EQUITY_FUND".to_string(),
                    fund_type: "Equity".to_string(),
                    fund_category: "Equity".to_string(),
                    expense_ratio: 0.0,
                    expense_ratio_date: NaiveDate::from_ymd_opt(2010, 1, 1).unwrap(),
                    aum: 100000000.0,
                    fund_rating: Some(5),
                    fund_rating_date: Some(NaiveDate::from_ymd_opt(2010, 1, 1).unwrap()),
                    category: "Equity".to_string(),
                }),
                "DEBT_FUND" => Ok(FundMetadata {
                    isin: "DEBT_FUND".to_string(),
                    fund_type: "Debt".to_string(),
                    fund_category: "Debt".to_string(),
                    expense_ratio: 0.0,
                    expense_ratio_date: NaiveDate::from_ymd_opt(2010, 1, 1).unwrap(),
                    aum: 100000000.0,
                    fund_rating: Some(4),
                    fund_rating_date: Some(NaiveDate::from_ymd_opt(2010, 1, 1).unwrap()),
                    category: "Debt".to_string(),
                }),
                _ => Err(anyhow::anyhow!("Unknown fund")),
            }
        }
    }

    // Mock price provider implementation for testing
    struct MockPriceProviderImpl;

    #[async_trait::async_trait]
    impl PriceProvider for MockPriceProviderImpl {
        async fn fetch_price(&self, symbol: &str) -> anyhow::Result<PriceResult> {
            let price = match symbol {
                "AAPL" => 150.0,
                "DEBT_FUND" => 100.0,
                _ => 0.0,
            };
            Ok(PriceResult {
                price,
                currency: "USD".to_string(),
                historical_prices: HashMap::new(),
                short_name: None,
            })
        }
    }

    #[tokio::test]
    async fn test_alloc_command() {
        let portfolios = vec![Portfolio {
            name: "Test".to_string(),
            investments: vec![
                Investment::Stock(StockInvestment {
                    symbol: "AAPL".to_string(),
                    units: 10.0,
                }),
                Investment::MutualFund(MutualFundInvestment {
                    isin: "EQUITY_FUND".to_string(),
                    units: 100.0,
                }),
                Investment::MutualFund(MutualFundInvestment {
                    isin: "DEBT_FUND".to_string(),
                    units: 50.0,
                }),
                Investment::FixedDeposit(FixedDepositInvestment {
                    name: "My FD".to_string(),
                    value: 5000.0,
                    currency: Some("USD".to_string()),
                }),
            ],
        }];

        let symbol_provider = MockPriceProviderImpl;
        let isin_provider = MockPriceProviderImpl;
        let currency_provider = MockCurrencyProvider;
        let metadata_provider = MockMetadataProviderImpl;

        let result = run(
            &portfolios,
            &symbol_provider,
            &isin_provider,
            &currency_provider,
            &metadata_provider,
            "USD",
        )
        .await;
        assert!(result.is_ok());
    }
}
