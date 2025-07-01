pub mod change;
pub mod config;
pub mod currency_provider;
pub mod log;
pub mod price_provider;
pub mod providers;
pub mod summary;
pub mod ui;

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
        .map_or("https://query1.finance.yahoo.com", |p| &p.base_url);
    let symbol_provider = crate::providers::caching::CachingPriceProvider::new(
        crate::providers::yahoo_finance::YahooFinanceProvider::new(base_url),
    );
    let currency_provider = crate::providers::caching::CachingCurrencyRateProvider::new(
        crate::providers::yahoo_finance::YahooCurrencyProvider::new(base_url),
    );

    let amfi_base_url = config
        .providers
        .amfi
        .as_ref()
        .map_or("https://mf.captnemo.in", |p| &p.base_url);
    let isin_provider = crate::providers::caching::CachingPriceProvider::new(
        crate::providers::amfi_provider::AmfiProvider::new(amfi_base_url),
    );

    summary::generate_and_display_summaries(
        &config.portfolios,
        &symbol_provider,
        &isin_provider,
        &currency_provider,
        &config.currency,
    )
    .await
}
