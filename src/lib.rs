pub mod cache;
pub mod change;
pub mod config;
pub mod currency_provider;
pub mod log;
pub mod price_provider;
pub mod providers;
pub mod returns;
pub mod summary;
pub mod ui;

use crate::price_provider::PriceResult;
use anyhow::Result;
use std::sync::Arc;
use tracing::{debug, info};

pub async fn run(config_path: Option<&str>) -> Result<()> {
    info!("Funds Tracker starting...");

    let config = match config_path {
        Some(path) => config::AppConfig::load_from_path(path)?,
        None => config::AppConfig::load()?,
    };
    debug!("Loaded config: {config:#?}");

    // Create shared caches
    let price_cache = Arc::new(cache::Cache::<String, PriceResult>::new());
    let rate_cache = Arc::new(cache::Cache::<String, f64>::new());

    let base_url = config
        .providers
        .yahoo
        .as_ref()
        .map_or("https://query1.finance.yahoo.com", |p| &p.base_url);
    let symbol_provider =
        providers::yahoo_finance::YahooFinanceProvider::new(base_url, Arc::clone(&price_cache));
    let currency_provider =
        providers::yahoo_finance::YahooCurrencyProvider::new(base_url, Arc::clone(&rate_cache));

    let amfi_base_url = config
        .providers
        .amfi
        .as_ref()
        .map_or("https://mf.captnemo.in", |p| &p.base_url);
    let isin_provider =
        providers::amfi_provider::AmfiProvider::new(amfi_base_url, Arc::clone(&price_cache));

    summary::generate_and_display_summaries(
        &config.portfolios,
        &symbol_provider,
        &isin_provider,
        &currency_provider,
        &config.currency,
    )
    .await
}
