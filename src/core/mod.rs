//! Core business logic abstractions

pub mod cache;
pub mod config;
pub mod currency;
pub mod log;
pub mod price;

// Re-export main types for cleaner imports
pub use currency::CurrencyRateProvider;
pub use price::{HistoricalPeriod, PriceProvider, PriceResult};
