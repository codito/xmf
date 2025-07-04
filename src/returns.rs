use crate::{
    cache::Cache,
    config::{AppConfig, Investment},
    price_provider::{HistoricalPeriod, PriceProvider, PriceResult},
    ui,
};
use anyhow::{Result, anyhow};
use comfy_table::Cell;
use futures::future::join_all;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tracing::{debug, info};

#[derive(Clone)]
struct ReturnResult {
    identifier: String,
    cagrs: BTreeMap<HistoricalPeriod, f64>, // Changed from single cagr to map
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
            Ok(price_data) => match calculate_cagr(&price_data) {
                Ok(cagrs) => return_results.push(ReturnResult {
                    identifier,
                    cagrs,
                    error: None,
                }),
                Err(e) => return_results.push(ReturnResult {
                    identifier,
                    cagrs: BTreeMap::new(),
                    error: Some(format!("CAGR calculation failed: {}", e)),
                }),
            },
            Err(e) => return_results.push(ReturnResult {
                identifier,
                cagrs: BTreeMap::new(),
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

fn calculate_cagr(price_data: &PriceResult) -> Result<BTreeMap<HistoricalPeriod, f64>> {
    let mut cagrs = BTreeMap::new();
    let periods = [
        HistoricalPeriod::OneDay,
        HistoricalPeriod::FiveDays,
        HistoricalPeriod::OneMonth,
        HistoricalPeriod::OneYear,
        HistoricalPeriod::ThreeYears,
        HistoricalPeriod::FiveYears,
        HistoricalPeriod::TenYears,
    ];

    for &period in &periods {
        if let Some(historical_price) = price_data.historical.get(&period) {
            let duration_years = period.to_duration().num_days() as f64 / 365.0;
            // Avoid division by zero if duration_years is 0, though for these periods it shouldn't be
            if duration_years > 0.0 {
                let cagr = ((price_data.price / historical_price).powf(1.0 / duration_years) - 1.0) * 100.0;
                cagrs.insert(period, cagr);
            }
        }
    }

    if cagrs.is_empty() {
        Err(anyhow!("No historical prices available for CAGR calculation"))
    } else {
        Ok(cagrs)
    }
}

fn display_return_results(results: &[ReturnResult]) {
    let mut table = ui::new_styled_table();
    let periods = [
        HistoricalPeriod::OneDay,
        HistoricalPeriod::FiveDays,
        HistoricalPeriod::OneMonth,
        HistoricalPeriod::OneYear,
        HistoricalPeriod::ThreeYears,
        HistoricalPeriod::FiveYears,
        HistoricalPeriod::TenYears,
    ];

    let mut header = vec![ui::header_cell("Identifier")];
    for period in &periods {
        header.push(ui::header_cell(&period.to_string()));
    }
    table.set_header(header);

    for result in results {
        let mut row_cells = vec![Cell::new(result.identifier.clone())];

        for period in &periods {
            let cell = match result.cagrs.get(period) {
                Some(cagr) => ui::change_cell(*cagr),
                None => ui::na_cell(result.error.is_some()),
            };
            row_cells.push(cell);
        }
        table.add_row(row_cells);
    }

    println!("{table}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::price_provider::{HistoricalPeriod, PriceResult};
    use std::collections::HashMap;

    fn create_test_data() -> PriceResult {
        PriceResult {
            price: 100.0,
            currency: "USD".to_string(),
            historical: HashMap::from([
                (HistoricalPeriod::OneDay, 99.0),
                (HistoricalPeriod::OneMonth, 95.0),
                (HistoricalPeriod::OneYear, 80.0),
                (HistoricalPeriod::ThreeYears, 50.0),
            ]),
            short_name: None,
        }
    }

    #[test]
    fn calculates_cagr_for_all_periods() {
        let data = create_test_data();
        let cagrs = calculate_cagr(&data).unwrap();

        assert_eq!(cagrs.len(), 4);
        assert!((cagrs[&HistoricalPeriod::OneDay] - 2537.04).abs() < 0.1);
        assert!((cagrs[&HistoricalPeriod::OneMonth] - 2633.52).abs() < 0.1);
        assert!((cagrs[&HistoricalPeriod::OneYear] - 25.0).abs() < 0.1);
        assert!((cagrs[&HistoricalPeriod::ThreeYears] - 25.99).abs() < 0.1);
    }

    #[test]
    fn handles_missing_historical_data() {
        let data = PriceResult {
            price: 100.0,
            currency: "USD".to_string(),
            historical: HashMap::new(),
            short_name: None,
        };

        assert!(calculate_cagr(&data).is_err());
    }
}
