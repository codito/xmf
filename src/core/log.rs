// Define a new module for logging initialization
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{
    EnvFilter, filter::Targets, fmt, prelude::__tracing_subscriber_SubscriberExt,
    util::SubscriberInitExt,
};

pub fn init_logging(verbose: bool) {
    let (level_filter, level) = if verbose {
        (LevelFilter::DEBUG, "debug")
    } else {
        (LevelFilter::OFF, "off")
    };
    let app_filter = Targets::new().with_target("xmf", level_filter);
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));

    tracing_subscriber::registry()
        .with(fmt::layer().pretty().without_time())
        .with(app_filter)
        .with(env_filter)
        .init();
}
