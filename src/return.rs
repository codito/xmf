use crate::{
    cache::Cache,
    config::{
        AppConfig, Investment, MutualFundInvestment, Purchase, StockInvestment, FixedDepositInvestment,
    },
    price_provider::{PriceProvider, PriceResult},
    ui,
};
use anyhow::{Result, anyhow};
use chrono::{NaiveDate, Utc, Datelike};
use comfy_table::Cell;
use futures::future::join_all;
use rust_finprim::rate::xirr;
use num_traits::cast::FromPrimitive;
use rust_finprim::Decimal;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info};

#[derive(Clone)]
struct XirrResult {
    identifier: String,
    xirr: Option<f64>,
    error: Option<String>,
}

pub async fn run(config_path: Option<&str>) -> Result<()> {
    info!("Calculating XIRR for investments...");

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
                    if !s.purchases.is_empty() {
                        investments_to_fetch
                            .insert(s.symbol.clone(), &stock_provider as &dyn PriceProvider);
                    }
                }
                Investment::MutualFund(mf) => {
                    if !mf.purchases.is_empty() {
                        investments_to_fetch
                            .insert(mf.isin.clone(), &mf_provider as &dyn PriceProvider);
                    }
                }
                Investment::FixedDeposit(_) => {} // Not relevant for XIRR
            }
        }
    }

    if investments_to_fetch.is_empty() {
        println!("No stock or mutual fund investments with purchase history found to calculate XIRR for.");
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

    let fetched_results: HashMap<String, Result<PriceResult>> = join_all(futures)
        .await
        .into_iter()
        .collect();
    pb.finish_and_clear();

    let mut xirr_results: Vec<XirrResult> = Vec::new();

    for portfolio in &config.portfolios {
        info!("Processing portfolio: {}", portfolio.name);
        for investment in &portfolio.investments {
            let (identifier, purchases) = match investment {
                Investment::Stock(s) => {
                    if s.purchases.is_empty() {
                        continue;
                    }
                    (s.symbol.clone(), &s.purchases)
                }
                Investment::MutualFund(mf) => {
                    if mf.purchases.is_empty() {
                        continue;
                    }
                    (mf.isin.clone(), &mf.purchases)
                }
                Investment::FixedDeposit(_) => continue, // Skip fixed deposits for XIRR
            };

            let current_price_result = fetched_results.get(&identifier).cloned();

            let xirr_res = match current_price_result {
                Some(Ok(price_result)) => {
                    if let Some(current_price) = price_result.current_price {
                        let total_units: f64 = purchases.iter().map(|p| p.units).sum();
                        let current_value = current_price * total_units;
                        match calculate_xirr(purchases, current_value) {
                            Ok(rate) => XirrResult {
                                identifier,
                                xirr: Some(rate),
                                error: None,
                            },
                            Err(e) => XirrResult {
                                identifier,
                                xirr: None,
                                error: Some(format!("XIRR calculation failed: {}", e)),
                            },
                        }
                    } else {
                        XirrResult {
                            identifier,
                            xirr: None,
                            error: Some("Current price not available".to_string()),
                        }
                    }
                }
                Some(Err(e)) => XirrResult {
                    identifier,
                    xirr: None,
                    error: Some(format!("Failed to fetch price: {}", e)),
                },
                None => XirrResult {
                    identifier,
                    xirr: None,
                    error: Some("Price data not found after fetch attempt".to_string()),
                },
            };
            xirr_results.push(xirr_res);
        }
    }

    if !xirr_results.is_empty() {
        display_xirr_results(&xirr_results);
    } else {
        println!("No XIRR results to display.");
    }

    Ok(())
}

fn calculate_xirr(purchases: &[Purchase], current_value: f64) -> Result<f64> {
    let mut cash_flows = vec![];
    let today_days = days_since_epoch(&Utc::now().date_naive());

    for purchase in purchases {
        let amount = Decimal::from_f64(-purchase.amount).ok_or_else(|| anyhow!("Invalid amount"))?
            .ok_or_else(|| anyhow!("Failed to convert purchase amount to Decimal"))?;
        cash_flows.push((amount, days_since_epoch(&purchase.date)));
    }

    // Add current value as positive cash flow
    let current_value_decimal = Decimal::from_f64(current_value).ok_or_else(|| anyhow!("Invalid current value"))?
        .ok_or_else(|| anyhow!("Failed to convert current value to Decimal"))?;
    cash_flows.push((current_value_decimal, today_days));

    if cash_flows.is_empty() {
        return Err(anyhow!("No cash flows to calculate XIRR"));
    }

    xirr(&cash_flows, None, Some(Decimal::from_f64(0.00001).unwrap()))
        .map_err(|e| anyhow!("XIRR computation error: {:?}", e))
}

fn days_since_epoch(date: &NaiveDate) -> i32 {
    date.num_days_from_ce() - NaiveDate::from_ymd_opt(1970, 1, 1).unwrap().num_days_from_ce()
}

fn display_xirr_results(results: &[XirrResult]) {
    let mut table = ui::new_styled_table();

    table.set_header(vec![ui::header_cell("Identifier"), ui::header_cell("XIRR (%)")]);

    for result in results {
        let xirr_cell = if let Some(rate) = result.xirr {
            let percentage = rate * 100.0;
            // Display positive XIRR in green, negative or zero in default/error
            if percentage > 0.0 {
                ui::value_cell(format!("{:.2}%", percentage), ui::StyleType::Success)
            } else {
                ui::value_cell(format!("{:.2}%", percentage), ui::StyleType::Default)
            }
        } else if let Some(error_msg) = &result.error {
            ui::na_cell(true)
        } else {
            ui::na_cell(false)
        };
        table.add_row(vec![Cell::new(result.identifier.clone()), xirr_cell]);
    }

    println!("{table}");
}
