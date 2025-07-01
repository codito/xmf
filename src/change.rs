use crate::config::{AppConfig, Investment};
use crate::price_provider::{HistoricalPeriod, PriceProvider};
use anyhow::Result;
use comfy_table::{presets::UTF8_FULL, modifiers::UTF8_ROUND_CORNERS, Attribute, Cell, Color, ContentArrangement, Table};
use futures::future::join_all;
use std::collections::BTreeMap;
use console::style;
use indicatif::{ProgressBar, ProgressStyle};
use std::collections::HashMap;

#[derive(Clone)]
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

    let mut investments_to_fetch = HashMap::new();
    for portfolio in &config.portfolios {
        for investment in &portfolio.investments {
            match investment {
                Investment::Stock(s) => {
                    investments_to_fetch.insert(s.symbol.clone(), &stock_provider as &dyn PriceProvider);
                }
                Investment::MutualFund(mf) => {
                    investments_to_fetch.insert(mf.isin.clone(), &mf_provider as &dyn PriceProvider);
                }
                Investment::FixedDeposit(_) => {}
            }
        }
    }

    if investments_to_fetch.is_empty() {
        println!("No stock or mutual fund investments found to display changes for.");
        return Ok(());
    }

    let pb = ProgressBar::new(investments_to_fetch.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})")?
            .progress_chars("#>-"),
    );

    let futures = investments_to_fetch
        .into_iter()
        .map(|(id, provider)| {
            let pb_clone = pb.clone();
            async move {
                let result = match provider.fetch_price(&id).await {
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
                    Err(e) => ChangeResult {
                        identifier: id,
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

    let num_portfolios = config.portfolios.len();
    for (i, portfolio) in config.portfolios.iter().enumerate() {
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
                style(&portfolio.name).bold().underlined()
            );
            display_results(&portfolio_results);

            if i < num_portfolios - 1 {
                let term_width = console::Term::stdout()
                    .size_checked()
                    .map(|(_, w)| w as usize)
                    .unwrap_or(80);
                println!("\n{}", "─".repeat(term_width));
            }
        }
    }

    Ok(())
}

fn display_results(results: &[ChangeResult]) {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS)
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
        let mut row_cells = vec![Cell::new(&result.identifier)];

        for period in &periods {
            match result.changes.get(period) {
                Some(change) => {
                    let cell = if *change >= 0.0 {
                        Cell::new(format!("{change:.2}%")).fg(Color::Green)
                    } else {
                        Cell::new(format!("{change:.2}%")).fg(Color::Red)
                    };
                    row_cells.push(cell);
                }
                None => {
                    let color = if result.error.is_some() {
                        Color::Red
                    } else {
                        Color::DarkGrey
                    };
                    row_cells.push(Cell::new("N/A").fg(color));
                }
            }
        }
        table.add_row(row_cells);
    }

    println!("{table}");
}
