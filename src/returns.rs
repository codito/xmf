use crate::{
    cache::Cache,
    config::{AppConfig, Investment},
    price_provider::{HistoricalPeriod, PriceProvider, PriceResult},
    ui,
};
use anyhow::{Result, anyhow};
use comfy_table::Cell;
use futures::future::join_all;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info};

#[derive(Clone)]
struct ReturnResult {
    identifier: String,
    cagr: Option<f64>,
    error: Option<String>,
}

pub async fn run(config_path: Option<&str>) -> Result<()> {
    info!("Calculating returns for investments...");

    let config = match config_path {
        Some(path) => AppConfig::load_from_path(path)?,
        None => AppConfig::load()?,
    };
    debug!("Loaded config: {config:#?}");

    let price_cache = Arc::new(Cache::<String, PriceResult>::new());

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

    let stock_provider = crate::providers::yahoo_finance::YahooFinanceProvider::new(
        base_url,
        Arc::clone(&price_cache),
    );
    let mf_provider =
        crate::providers::amfi_provider::AmfiProvider::new(amfi_base_url, Arc::clone(&price_cache));

    let mut investments_to_fetch = HashMap::new();
    for portfolio in &config.portfolios {
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
                Investment::FixedDeposit(_) => {} // Not relevant for returns
            }
        }
    }

    if investments_to_fetch.is_empty() {
        println!("No investments found to calculate returns for.");
        return Ok(());
    }

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

    let mut return_results: Vec<ReturnResult> = Vec::new();

    for (identifier, price_result) in fetched_results {
        match price_result {
            Ok(price_data) => {
                match calculate_cagr(&price_data) {
                    Ok(cagr) => return_results.push(ReturnResult {
                        identifier,
                        cagr: Some(cagr),
                        error: None,
                    }),
                    Err(e) => return_results.push(ReturnResult {
                        identifier,
                        cagr: None,
                        error: Some(format!("CAGR calculation failed: {}", e)),
                    }),
                }
            }
            Err(e) => return_results.push(ReturnResult {
                identifier,
                cagr: None,
                error: Some(format!("Price fetch failed: {}", e)),
            }),
        }
    }

    if !return_results.is_empty() {
        display_return_results(&return_results);
    } else {
        println!("No return results to display.");
    }

    Ok(())
}

fn calculate_cagr(price_data: &PriceResult) -> Result<f64> {
    // Use 3-year historical price by default
    let period = HistoricalPeriod::ThreeYears;
    
    if let Some(period_price) = price_data.historical.get(&period) {
        let current_price = price_data
            .price
            .ok_or_else(|| anyhow!("Current price missing"))?;

        // Calculate time period in years
        let duration_years = period.to_duration().num_days() as f64 / 365.0;
        
        // Calculate CAGR: [(current / historical)^(1/years) - 1] * 100
        let cagr = ((current_price / period_price).powf(1.0 / duration_years) - 1.0) * 100.0;
        Ok(cagr)
    } else {
        Err(anyhow!("Historic price unavailable for {} period", period))
    }
}

// Format: "CAGR" instead of "XIRR"
fn display_return_results(results: &[ReturnResult]) {
    let mut table = ui::new_styled_table();

    table.set_header(vec![
        ui::header_cell("Identifier"),
        ui::header_cell("CAGR (3Y, %)"),
    ]);

    for result in results {
        let cagr_cell = if let Some(rate) = result.cagr {
            ui::change_cell(rate)
        } else if let Some(_) = &result.error {
            ui::na_cell(true)
        } else {
            ui::na_cell(false)
        };
        table.add_row(vec![
            Cell::new(result.identifier.clone()),
            cagr_cell,
        ]);
    }

    println!("{table}");
}
