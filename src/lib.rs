pub mod cli;
pub mod core;
pub mod providers;
pub mod store;

use crate::store::KeyValueStore;
use anyhow::Result;
use std::sync::Arc;
use tracing::{debug, info};

/// Commands that require full provider setup
pub enum AppCommand {
    Summary,
    Change,
    Returns,
    Fees,
    Alloc,
}

/// Common command execution entry point
pub async fn run_command(
    command: AppCommand,
    config_path: Option<&std::path::Path>,
    force_refresh: bool,
) -> Result<()> {
    info!("Funds Tracker starting...");

    let config = match config_path {
        Some(path) => core::config::AppConfig::load_from_path(path)?,
        None => core::config::AppConfig::load()?,
    };
    debug!("Loaded config: {config:#?}");

    // Create shared caches
    let data_path = config
        .default_data_path()
        .expect("Failed to get default data path");
    let store = Arc::new(KeyValueStore::new(data_path.as_path()));

    if force_refresh {
        info!("--refresh: clearing persistent cache");
        store.clear_persistent_cache()?;
    }

    // Initialize providers
    let (symbol_provider, isin_provider, currency_provider, metadata_provider) =
        setup_providers(&config, &store);

    match command {
        AppCommand::Summary => {
            cli::summary::run(
                &config.portfolios,
                &*symbol_provider,
                &*isin_provider,
                &*currency_provider,
                &config.currency,
            )
            .await
        }
        AppCommand::Change => {
            cli::change::run(
                &config.portfolios,
                &*symbol_provider,
                &*isin_provider,
                &*currency_provider,
                &config.currency,
            )
            .await
        }
        AppCommand::Returns => {
            cli::returns::run(
                &config.portfolios,
                &*symbol_provider,
                &*isin_provider,
                &*currency_provider,
                &config.currency,
            )
            .await
        }
        AppCommand::Fees => {
            cli::fees::run(
                &config.portfolios,
                &*symbol_provider,
                &*isin_provider,
                &*currency_provider,
                &*metadata_provider,
                &config.currency,
            )
            .await
        }
        AppCommand::Alloc => {
            cli::alloc::run(
                &config.portfolios,
                &*symbol_provider,
                &*isin_provider,
                &*currency_provider,
                &*metadata_provider,
                &config.currency,
            )
            .await
        }
    }
}

fn setup_providers(
    config: &core::config::AppConfig,
    store: &Arc<KeyValueStore>,
) -> (
    Arc<providers::yahoo_finance::YahooFinanceProvider>,
    Arc<providers::amfi_provider::AmfiProvider>,
    Arc<providers::yahoo_finance::YahooCurrencyProvider>,
    Arc<providers::kuvera_provider::KuveraProvider>,
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
            Arc::clone(store),
        )),
        Arc::new(providers::amfi_provider::AmfiProvider::new(
            amfi_base,
            Arc::clone(store),
        )),
        Arc::new(providers::yahoo_finance::YahooCurrencyProvider::new(
            yahoo_base,
            Arc::clone(store),
        )),
        Arc::new(providers::kuvera_provider::KuveraProvider::new(
            amfi_base,
            Arc::clone(store),
        )),
    )
}
