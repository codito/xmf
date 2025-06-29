// Define a new module for logging initialization
use tracing_subscriber::{
    EnvFilter, fmt, prelude::__tracing_subscriber_SubscriberExt, util::SubscriberInitExt,
};

pub fn init_logging(verbose: bool) {
    let level = if verbose { "debug" } else { "off" };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));

    tracing_subscriber::registry()
        .with(fmt::layer().pretty().without_time())
        .with(filter)
        .init();
}
