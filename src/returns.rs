use crate::{
    config::Investment,
    core::{CurrencyRateProvider, HistoricalPeriod, PriceProvider, PriceResult},
    ui,
};
use anyhow::{Result, anyhow};
use comfy_table::Cell;
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

pub async fn run(
    portfolios: &[crate::config::Portfolio],
    symbol_provider: &(dyn PriceProvider + Send + Sync),
    isin_provider: &(dyn PriceProvider + Send + Sync),
    _currency_provider: &(dyn CurrencyRateProvider + Send + Sync),
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
        debug!("Calculating CAGR for {identifier}");
        match price_result {
            Ok(price_data) => match calculate_cagr(&price_data) {
                Ok(cagrs) => return_results.push(ReturnResult {
                    identifier,
                    short_name: price_data.short_name.clone(),
                    cagrs,
                    error: None,
                }),
                Err(e) => return_results.push(ReturnResult {
                    identifier,
                    short_name: None,
                    cagrs: BTreeMap::new(),
                    error: Some(format!("CAGR calculation failed: {e}")),
                }),
            },
            Err(e) => return_results.push(ReturnResult {
                identifier,
                short_name: None,
                cagrs: BTreeMap::new(),
                error: Some(format!("Price fetch failed: {e}")),
            }),
        }
    }

    let return_results_map: HashMap<String, ReturnResult> = return_results
        .into_iter()
        .map(|r| (r.identifier.clone(), r))
        .collect();

    let num_portfolios = portfolios.len();
    for (i, portfolio) in portfolios.iter().enumerate() {
        let portfolio_results: Vec<ReturnResult> = portfolio
            .investments
            .iter()
            .filter_map(|investment| match investment {
                Investment::Stock(s) => return_results_map.get(&s.symbol).cloned(),
                Investment::MutualFund(mf) => return_results_map.get(&mf.isin).cloned(),
                Investment::FixedDeposit(_) => None,
            })
            .collect();

        if !portfolio_results.is_empty() {
            println!(
                "\nPortfolio: {}",
                ui::style_text(&portfolio.name, ui::StyleType::Title)
            );
            display_return_results(&portfolio_results);

            if i < num_portfolios - 1 {
                ui::print_separator();
            }
        }
    }

    if return_results_map.is_empty() {
        println!("No return results to display.");
    }

    Ok(())
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

fn display_return_results(results: &[ReturnResult]) {
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

    for result in results {
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

    println!("{table}");
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
