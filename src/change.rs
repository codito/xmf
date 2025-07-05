use crate::cache::Cache;
use crate::config::{AppConfig, Investment};
use crate::price_provider::{HistoricalPeriod, PriceProvider, PriceResult};
use crate::ui;
use anyhow::Result;
use comfy_table::Cell;
use futures::future::join_all;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone)]
struct ChangeResult {
    identifier: String,
    short_name: Option<String>,
    changes: BTreeMap<HistoricalPeriod, f64>,
    error: Option<String>,
}

pub async fn run(
    portfolios: &[crate::config::Portfolio],
    symbol_provider: &(dyn crate::price_provider::PriceProvider + Send + Sync),
    isin_provider: &(dyn crate::price_provider::PriceProvider + Send + Sync),
    _currency_provider: &(dyn crate::currency_provider::CurrencyRateProvider + Send + Sync),
) -> anyhow::Result<()> {

    let mut investments_to_fetch = HashMap::new();
    for portfolio in portfolios {
        for investment in &portfolio.investments {
            match investment {
                Investment::Stock(s) => {
                    investments_to_fetch
                        .insert(s.symbol.clone(), &stock_provider as &dyn PriceProvider);
                }
                Investment::MutualFund(mf) => {
                    investments_to_fetch
                        .insert(mf.isin.clone(), &mf_provider as &dyn PriceProvider);
                }
                Investment::FixedDeposit(_) => {}
            }
        }
    }

    if investments_to_fetch.is_empty() {
        println!("No stock or mutual fund investments found to display changes for.");
        return Ok(());
    }

    let pb = ui::new_progress_bar(investments_to_fetch.len() as u64, false);

    let futures = investments_to_fetch.into_iter().map(|(id, provider)| {
        let pb_clone = pb.clone();
        async move {
            let result = match provider.fetch_price(&id).await {
                Ok(price_result) => {
                    let mut changes = BTreeMap::new();
                    for (period, historical_price) in &price_result.historical_prices {
                        if *historical_price > 0.0 {
                            let change = ((price_result.price - historical_price)
                                / historical_price)
                                * 100.0;
                            changes.insert(*period, change);
                        }
                    }
                    ChangeResult {
                        identifier: id,
                        short_name: price_result.short_name,
                        changes,
                        error: None,
                    }
                }
                Err(e) => ChangeResult {
                    identifier: id,
                    short_name: None,
                    changes: BTreeMap::new(),
                    error: Some(e.to_string()),
                },
            };
            pb_clone.inc(1);
            result
        }
    });

    let results: Vec<ChangeResult> = join_all(futures).await;
    pb.finish_and_clear();

    let results_map: HashMap<String, ChangeResult> = results
        .into_iter()
        .map(|r| (r.identifier.clone(), r))
        .collect();

    let num_portfolios = portfolios.len();
    for (i, portfolio) in portfolios.iter().enumerate() {
        let portfolio_results: Vec<ChangeResult> = portfolio
            .investments
            .iter()
            .filter_map(|investment| match investment {
                Investment::Stock(s) => results_map.get(&s.symbol).cloned(),
                Investment::MutualFund(mf) => results_map.get(&mf.isin).cloned(),
                Investment::FixedDeposit(_) => None,
            })
            .collect();

        if !portfolio_results.is_empty() {
            println!(
                "\nPortfolio: {}",
                ui::style_text(&portfolio.name, ui::StyleType::Title)
            );
            display_results(&portfolio_results);

            if i < num_portfolios - 1 {
                ui::print_separator();
            }
        }
    }

    Ok(())
}

fn display_results(results: &[ChangeResult]) {
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

    for result in results {
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

    println!("{table}");
}
