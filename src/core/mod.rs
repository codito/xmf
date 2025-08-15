//! Core business logic abstractions

pub mod allocation;
pub mod analytics;
pub mod cache;
pub mod config;
pub mod currency;
pub mod log;
pub mod metadata;
pub mod price;

// Re-export main types for cleaner imports
pub use currency::CurrencyRateProvider;
pub use metadata::{FundMetadata, MetadataProvider};
pub use price::{HistoricalPeriod, PriceProvider, PriceResult};
