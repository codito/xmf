use super::ui;
use crate::core::analytics;
use crate::core::config::{Investment, Portfolio};
use crate::core::currency::CurrencyRateProvider;
use crate::core::metadata::{FundMetadata, MetadataProvider};
use crate::core::price::{PriceProvider, PriceResult};
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
    // Collect all price identifiers and metadata ISINs first
    let mut price_fetch_map = HashMap::new();
    let mut metadata_isins = Vec::new();

    for portfolio in portfolios {
        for investment in &portfolio.investments {
            match investment {
                Investment::Stock(s) => {
                    price_fetch_map.insert(s.symbol.clone(), symbol_provider);
                }
                Investment::MutualFund(mf) => {
                    price_fetch_map.insert(mf.isin.clone(), isin_provider);
                    metadata_isins.push(mf.isin.clone());
                }
                Investment::FixedDeposit(_) => {}
            }
        }
    }

    // Early return if there's nothing to fetch
    if price_fetch_map.is_empty() && metadata_isins.is_empty() {
        println!("No investments to display fees for.");
        return Ok(());
    }

    // Step 1: Fetch all prices with progress bar
    let pb_price = if !price_fetch_map.is_empty() {
        let pb = ui::new_progress_bar(price_fetch_map.len() as u64, true);
        pb.set_message("Fetching prices...");
        Some(pb)
    } else {
        None
    };

    let price_futures = price_fetch_map.iter().map(|(id, provider)| {
        let pb_clone = pb_price.clone();
        async move {
            let res = provider.fetch_price(id).await;
            if let Some(pb) = pb_clone {
                pb.inc(1);
            }
            (id.clone(), res)
        }
    });

    let price_results: HashMap<String, Result<PriceResult>> =
        join_all(price_futures).await.into_iter().collect();

    if let Some(pb) = pb_price {
        pb.finish_and_clear();
    }

    // Step 2: Fetch metadata with progress bar
    let pb_metadata = if !metadata_isins.is_empty() {
        let pb = ui::new_progress_bar(metadata_isins.len() as u64, true);
        pb.set_message("Fetching metadata...");
        Some(pb)
    } else {
        None
    };

    let metadata_futures = metadata_isins.into_iter().map(|isin| {
        let pb_clone = pb_metadata.clone();
        async move {
            let res = metadata_provider.fetch_metadata(&isin).await;
            if let Some(pb) = pb_clone {
                pb.inc(1);
            }
            (isin, res)
        }
    });

    let metadata_results: HashMap<String, Result<FundMetadata>> =
        join_all(metadata_futures).await.into_iter().collect();

    if let Some(pb) = pb_metadata {
        pb.finish_and_clear();
    }

    // Process each portfolio with the pre-fetched data
    for (i, portfolio) in portfolios.iter().enumerate() {
        let holdings = analytics::calculate_portfolio_value(
            portfolio,
            &price_results,
            currency_provider,
            target_currency,
            &|| {},
        )
        .await;

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
    metadata_results: &HashMap<String, Result<FundMetadata>>,
) -> PortfolioFeeResult {
    let mut investment_fees = Vec::new();
    let mut total_weighted_fee = 0.0;
    let mut total_weight = 0.0;

    for (inv_index, investment) in portfolio.investments.iter().enumerate() {
        if inv_index >= holdings.investments.len() {
            continue;
        }

        let holding = &holdings.investments[inv_index];
        let weight = holding.weight.unwrap_or(0.0);
        let mut expense_ratio = 0.0;
        let mut error = None;

        if let Investment::MutualFund(mf) = investment
            && let Some(result) = metadata_results.get(&mf.isin)
        {
            match result {
                Ok(meta) => expense_ratio = meta.expense_ratio,
                Err(e) => error = Some(e.to_string()),
            }
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
        ui::header_cell("Expense Ratio (%)"),
        ui::header_cell("Weight (%)"),
    ]);

    for fee_result in &result.investment_fees {
        let name_display = if let Some(name) = &fee_result.short_name {
            name.clone()
        } else {
            fee_result.identifier.clone()
        };

        let expense_cell = if let Some(err) = &fee_result.error {
            Cell::new(format!("Error: {err}")).fg(comfy_table::Color::Red)
        } else {
            ui::format_optional_cell(Some(fee_result.expense_ratio), |v| format!("{:.2}", v))
        };

        table.add_row(vec![
            Cell::new(name_display),
            expense_cell,
            ui::format_optional_cell(Some(fee_result.weight), |v| format!("{:.2}", v)),
        ]);
    }

    // Add portfolio summary row
    if !result.investment_fees.is_empty() {
        table.add_row(vec![
            Cell::new("Portfolio Weighted").add_attribute(Attribute::Bold),
            ui::format_percentage_cell(result.portfolio_fee, |v| format!("{:.2}", v)),
            Cell::new("100.0")
                .add_attribute(Attribute::Bold)
                .set_alignment(comfy_table::CellAlignment::Right),
        ]);
    }

    println!("{table}");
}
