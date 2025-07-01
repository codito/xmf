pub mod config;
pub mod currency_provider;
pub mod log;
pub mod price_provider;
pub mod providers;
pub mod summary; // New module declaration

use anyhow::Result;
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

    let amfi_base_url = config
        .providers
        .amfi
        .as_ref()
        .map(|c| c.base_url.as_str())
        .unwrap_or("https://mf.captnemo.in");

    let provider = crate::providers::yahoo_finance::YahooFinanceProvider::new(base_url);
    let amfi_provider = crate::providers::amfi_provider::AmfiProvider::new(amfi_base_url);
    let currency_provider = crate::providers::yahoo_finance::YahooCurrencyProvider::new(base_url);

    summary::generate_and_display_summaries(
        &config.portfolios,
        &provider,
        &amfi_provider,
        &currency_provider,
        &config.currency,
    )
    .await
}
