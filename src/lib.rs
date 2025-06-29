pub mod config;
pub mod currency_provider;
pub mod log;
pub mod price_provider;
pub mod providers;
pub mod summary; // New module declaration

use crate::providers::yahoo_finance::{YahooCurrencyProvider, YahooFinanceProvider};
use anyhow::Result;
use std::collections::HashMap;
use tracing::{debug, info};

pub async fn run(config_path: Option<&str>) -> Result<()> {
    info!("Funds Tracker starting...");

    let config = match config_path {
        Some(path) => config::AppConfig::load_from_path(path)?,
        None => config::AppConfig::load()?,
    };
    debug!("Loaded config: {config:#?}");

    let base_url = config
        .providers
        .yahoo
        .as_ref()
        .map(|c| c.base_url.as_str())
        .unwrap_or("https://query1.finance.yahoo.com");

    let currency_base_url = config
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

    let provider = YahooFinanceProvider::new(base_url);
    let amfi_provider = providers::amfi_provider::AmfiProvider::new(amfi_base_url);
    let currency_provider = YahooCurrencyProvider::new(currency_base_url);
    let mut price_cache = HashMap::new();

    for portfolio in &config.portfolios {
        let sum = summary::generate_portfolio_summary(
            portfolio,
            &provider,
            &amfi_provider,
            &currency_provider,
            &mut price_cache,
            &config.currency,
        )
        .await;

        println!("{}", sum.display_as_table());
    }

    Ok(())
}
