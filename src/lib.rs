pub mod cache;
pub mod change;
pub mod config;
pub mod core;
pub mod log;
pub mod providers;
pub mod returns;
pub mod summary;
pub mod ui;

use crate::price_provider::PriceResult;
use anyhow::Result;
use std::sync::Arc;
use tracing::{debug, info};

/// Commands that require full provider setup
pub enum AppCommand {
    Summary,
    Change,
    Returns,
}

/// Common command execution entry point
pub async fn run_command(command: AppCommand, config_path: Option<&str>) -> Result<()> {
    info!("Funds Tracker starting...");

    let config = match config_path {
        Some(path) => config::AppConfig::load_from_path(path)?,
        None => config::AppConfig::load()?,
    };
    debug!("Loaded config: {config:#?}");

    // Create shared caches
    let price_cache = Arc::new(cache::Cache::<String, PriceResult>::new());
    let rate_cache = Arc::new(cache::Cache::<String, f64>::new());

    // Initialize providers
    let (symbol_provider, isin_provider, currency_provider) =
        setup_providers(&config, &price_cache, &rate_cache);

    match command {
        AppCommand::Summary => {
            summary::run(
                &config.portfolios,
                &*symbol_provider,
                &*isin_provider,
                &*currency_provider,
                &config.currency,
            )
            .await
        }
        AppCommand::Change => {
            change::run(
                &config.portfolios,
                &*symbol_provider,
                &*isin_provider,
                &*currency_provider,
            )
            .await
        }
        AppCommand::Returns => {
            returns::run(
                &config.portfolios,
                &*symbol_provider,
                &*isin_provider,
                &*currency_provider,
            )
            .await
        }
    }
}

fn setup_providers(
    config: &config::AppConfig,
    price_cache: &Arc<cache::Cache<String, PriceResult>>,
    rate_cache: &Arc<cache::Cache<String, f64>>,
) -> (
    Arc<providers::yahoo_finance::YahooFinanceProvider>,
    Arc<providers::amfi_provider::AmfiProvider>,
    Arc<providers::yahoo_finance::YahooCurrencyProvider>,
) {
    let yahoo_base = config
        .providers
        .yahoo
        .as_ref()
        .map_or("https://query1.finance.yahoo.com", |p| &p.base_url);

    let amfi_base = config
        .providers
        .amfi
        .as_ref()
        .map_or("https://mf.captnemo.in", |p| &p.base_url);

    (
        Arc::new(providers::yahoo_finance::YahooFinanceProvider::new(
            yahoo_base,
            Arc::clone(price_cache),
        )),
        Arc::new(providers::amfi_provider::AmfiProvider::new(
            amfi_base,
            Arc::clone(price_cache),
        )),
        Arc::new(providers::yahoo_finance::YahooCurrencyProvider::new(
            yahoo_base,
            Arc::clone(rate_cache),
        )),
    )
}
