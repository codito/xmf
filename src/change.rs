use crate::config::{AppConfig, Investment};
use crate::price_provider::{HistoricalPeriod, PriceProvider};
use anyhow::Result;
use comfy_table::{presets::UTF8_FULL, Attribute, Cell, Color, ContentArrangement, Table};
use futures::future::join_all;
use std::collections::BTreeMap;
use tracing::error;

struct ChangeResult {
    identifier: String,
    changes: BTreeMap<HistoricalPeriod, f64>,
    error: Option<String>,
}

pub async fn run(config_path: Option<&str>) -> Result<()> {
    let config = match config_path {
        Some(path) => AppConfig::load_from_path(path)?,
        None => AppConfig::load()?,
    };

    let base_url = config
        .providers
        .yahoo
        .as_ref()
        .map(|c| c.base_url.as_str())
        .unwrap_or("https://query1.finance.yahoo.com");

    let amfi_base_url = config
        .providers
        .amfi
        .as_ref()
        .map(|c| c.base_url.as_str())
        .unwrap_or("https://mf.captnemo.in");

    let stock_provider = crate::providers::yahoo_finance::YahooFinanceProvider::new(base_url);
    let mf_provider = crate::providers::amfi_provider::AmfiProvider::new(amfi_base_url);

    let mut all_investments: Vec<(String, &dyn PriceProvider)> = vec![];
    for portfolio in &config.portfolios {
        for investment in &portfolio.investments {
            match investment {
                Investment::Stock(s) => {
                    all_investments.push((s.symbol.clone(), &stock_provider));
                }
                Investment::MutualFund(mf) => {
                    all_investments.push((mf.isin.clone(), &mf_provider));
                }
                Investment::FixedDeposit(_) => {}
            }
        }
    }

    let futures = all_investments
        .into_iter()
        .map(|(id, provider)| async move {
            match provider.fetch_price(&id).await {
                Ok(price_result) => {
                    let mut changes = BTreeMap::new();
                    for (period, change) in price_result.historical {
                        changes.insert(period, change);
                    }
                    ChangeResult {
                        identifier: id,
                        changes,
                        error: None,
                    }
                }
                Err(e) => {
                    error!("Failed to fetch price for {}: {}", id, e);
                    ChangeResult {
                        identifier: id,
                        changes: BTreeMap::new(),
                        error: Some(e.to_string()),
                    }
                }
            }
        });

    let results: Vec<ChangeResult> = join_all(futures).await;

    display_results(&results);

    Ok(())
}

fn display_results(results: &[ChangeResult]) {
    if results.is_empty() {
        println!("No investments found to display changes for.");
        return;
    }

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic);

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

    let mut header = vec![Cell::new("Identifier")
        .fg(Color::Cyan)
        .add_attribute(Attribute::Bold)];
    for period in &periods {
        header.push(
            Cell::new(period.to_string())
                .fg(Color::Cyan)
                .add_attribute(Attribute::Bold),
        );
    }
    table.set_header(header);

    for result in results {
        let mut row = vec![Cell::new(&result.identifier)];
        if let Some(e) = &result.error {
            let error_cell = Cell::new(format!("Error: {e}"))
                .set_col_span(periods.len())
                .fg(Color::Red);
            row.push(error_cell);
        } else {
            for period in &periods {
                match result.changes.get(period) {
                    Some(change) => {
                        let cell = if *change >= 0.0 {
                            Cell::new(format!("{change:.2}%")).fg(Color::Green)
                        } else {
                            Cell::new(format!("{change:.2}%")).fg(Color::Red)
                        };
                        row.push(cell);
                    }
                    None => row.push(Cell::new("N/A").fg(Color::DarkGrey)),
                }
            }
        }
        table.add_row(row);
    }

    println!("{table}");
}
