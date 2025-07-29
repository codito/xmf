use super::ui;
use crate::core::analytics;
use crate::core::config::{Investment, Portfolio};
use crate::core::currency::CurrencyRateProvider;
use crate::core::metadata::{FundMetadata, MetadataProvider};
use crate::core::price::PriceProvider;
use anyhow::Result;
use comfy_table::{Attribute, Cell};
use futures::future::join_all;
use std::collections::HashMap;

#[derive(Clone)]
struct FeeResult {
    identifier: String,
    short_name: Option<String>,
    expense_ratio: f64,
    weight: f64,
    error: Option<String>,
}

struct PortfolioFeeResult {
    name: String,
    investment_fees: Vec<FeeResult>,
    portfolio_fee: f64,
}

pub async fn run(
    portfolios: &[Portfolio],
    symbol_provider: &(dyn PriceProvider + Send + Sync),
    isin_provider: &(dyn PriceProvider + Send + Sync),
    currency_provider: &(dyn CurrencyRateProvider + Send + Sync),
    metadata_provider: &(dyn MetadataProvider + Send + Sync),
    target_currency: &str,
) -> anyhow::Result<()> {
    // Prepare all identifiers
    let mut investments_to_process = Vec::new();
    for portfolio in portfolios {
        for investment in &portfolio.investments {
            investments_to_process.push(investment.clone());
        }
    }

    // Get current portfolio values for weighting
    let price_futures = investments_to_process.iter().filter_map(|inv| {
        let provider = match inv {
            Investment::Stock(s) => Some((s.symbol.clone(), symbol_provider)),
            Investment::MutualFund(mf) => Some((mf.isin.clone(), isin_provider)),
            Investment::FixedDeposit(_) => None,
        };
        provider
    });

    let num_price_futures = price_futures.len();
    let pb_price = if num_price_futures > 0 {
        let pb = ui::new_progress_bar(num_price_futures as u64, false);
        Some(pb)
    } else {
        None
    };

    let price_results = if num_price_futures > 0 {
        let futures = price_futures.into_iter().map(|(id, provider)| {
            let pb_clone = pb_price.clone().unwrap();
            async move {
                let result = provider.fetch_price(&id).await;
                pb_clone.inc(1);
                (id, result)
            }
        });
        let results = join_all(futures)
            .await
            .into_iter()
            .collect::<HashMap<_, _>>();
        pb_price.unwrap().finish_and_clear();
        results
    } else {
        HashMap::new()
    };

    // Process each portfolio
    for (i, portfolio) in portfolios.iter().enumerate() {
        let holdings = analytics::calculate_portfolio_value(
            portfolio,
            &price_results,
            currency_provider,
            target_currency,
            &|| (), // No progress updates
        )
        .await;

        if investments_to_process.is_empty() {
            println!("No investments to display fees for.");
            return Ok(());
        }

        // Prepare mutual fund metadata fetches
        let investments_to_fetch = portfolio.investments.iter().filter_map(|inv| {
            if let Investment::MutualFund(mf) = inv {
                Some(mf.isin.as_str())
            } else {
                None
            }
        });

        let num_metadata = investments_to_fetch.clone().count();
        let pb_metadata = if num_metadata > 0 {
            let pb = ui::new_progress_bar(num_metadata as u64, false);
            Some(pb)
        } else {
            None
        };

        let metadata_futures = investments_to_fetch.map(|isin| {
            let pb_clone = pb_metadata.clone();
            async move {
                let result = metadata_provider.fetch_metadata(isin).await;
                if let Some(pb) = pb_clone {
                    pb.inc(1);
                }
                result
            }
        });

        let metadata_results = join_all(metadata_futures).await;
        if let Some(pb) = pb_metadata {
            pb.finish_and_clear();
        }

        let result = calculate_portfolio_fees(portfolio, &holdings, &metadata_results).await;

        println!(
            "\nPortfolio: {}",
            ui::style_text(&result.name, ui::StyleType::Title)
        );
        display_results(&result);

        if i < portfolios.len() - 1 {
            ui::print_separator();
        }
    }

    Ok(())
}

async fn calculate_portfolio_fees(
    portfolio: &Portfolio,
    holdings: &analytics::PortfolioValue,
    metadata_results: &[Result<FundMetadata>],
) -> PortfolioFeeResult {
    let mut investment_fees = Vec::new();
    let mut total_weighted_fee = 0.0;
    let mut total_weight = 0.0;
    let mut metadata_index = 0;

    for (inv_index, investment) in portfolio.investments.iter().enumerate() {
        if inv_index >= holdings.investments.len() {
            continue;
        }

        let holding = &holdings.investments[inv_index];
        let weight = holding.weight.unwrap_or(0.0);
        let mut expense_ratio = 0.0;
        let mut error = None;

        match investment {
            Investment::MutualFund(_) if metadata_index < metadata_results.len() => {
                match &metadata_results[metadata_index] {
                    Ok(meta) => expense_ratio = meta.expense_ratio,
                    Err(e) => error = Some(e.to_string()),
                }
                metadata_index += 1;
            }
            _ => {} // Non-mutual funds have 0.0 fee
        }

        total_weight += weight;
        total_weighted_fee += expense_ratio * weight;

        investment_fees.push(FeeResult {
            identifier: holding.identifier.clone(),
            short_name: holding.short_name.clone(),
            expense_ratio,
            weight,
            error,
        });
    }

    PortfolioFeeResult {
        name: portfolio.name.clone(),
        investment_fees,
        portfolio_fee: if total_weight > 0.0 {
            total_weighted_fee / 100.0
        } else {
            0.0
        },
    }
}

fn display_results(result: &PortfolioFeeResult) {
    let mut table = ui::new_styled_table();

    table.set_header(vec![
        ui::header_cell("Investment"),
        ui::header_cell("Expense Ratio"),
        ui::header_cell("Weight"),
    ]);

    for fee_result in &result.investment_fees {
        let identifier = if let Some(name) = &fee_result.short_name {
            name.clone()
        } else {
            fee_result.identifier.clone()
        };

        let expense_cell = if let Some(err) = &fee_result.error {
            Cell::new(format!("Error: {err}")).fg(comfy_table::Color::Red)
        } else {
            Cell::new(format!("{:.2}%", fee_result.expense_ratio))
        };

        table.add_row(vec![
            Cell::new(identifier),
            expense_cell,
            Cell::new(format!("{:.1}%", fee_result.weight)),
        ]);
    }

    // Add portfolio summary row
    if !result.investment_fees.is_empty() {
        table.add_row(vec![
            Cell::new("Portfolio Weighted").add_attribute(Attribute::Bold),
            ui::format_percentage_cell(result.portfolio_fee),
            Cell::new("100.0%").add_attribute(Attribute::Bold),
        ]);
    }

    println!("{table}");
}
