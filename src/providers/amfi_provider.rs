use crate::core::cache::{KeyValueCollection, Store};
use crate::core::{HistoricalPeriod, PriceProvider, PriceResult};
use crate::providers::util::{seconds_until, with_retry};
use crate::store::KeyValueStore;
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use chrono;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};

pub struct AmfiProvider {
    base_url: String,
    cache: Arc<dyn KeyValueCollection>,
}

impl AmfiProvider {
    pub fn new(base_url: &str, cache: Arc<KeyValueStore>) -> Self {
        let collection = cache.get_collection("amfi", true, true).unwrap();
        AmfiProvider {
            base_url: base_url.to_string(),
            cache: collection,
        }
    }

    #[cfg(test)]
    pub(crate) fn new_with_collection(base_url: &str, cache: Arc<dyn KeyValueCollection>) -> Self {
        Self {
            base_url: base_url.to_string(),
            cache,
        }
    }
}

#[derive(Debug, Deserialize)]
struct AmfiResponse {
    nav: f64,
    date: String,
    name: Option<String>,
    #[serde(default)]
    historical_nav: Vec<(String, f64)>,
}

#[async_trait]
impl PriceProvider for AmfiProvider {
    async fn fetch_price(&self, identifier: &str) -> Result<PriceResult> {
        if let Some(cached) = self.cache.get(identifier.as_bytes()).await {
            return Ok(serde_json::from_slice(&cached)?);
        }

        let url = format!("{}/nav/{}", self.base_url, identifier);
        debug!("Requesting price data from {}", url);

        let client = reqwest::Client::builder().user_agent("xmf/1.0").build()?;
        let response = with_retry(|| async { client.get(&url).send().await }, 3, 500)
            .await
            .with_context(|| format!("Failed to send request for ISIN: {identifier}"))?;

        let response_text = response
            .text()
            .await
            .with_context(|| format!("Failed to get response text for ISIN: {identifier}"))?;

        // Check for empty or non-JSON responses before parsing
        if response_text.trim().is_empty() {
            return Err(anyhow!("Received empty response for ISIN: {}", identifier));
        }

        let amfi_response: AmfiResponse =
            serde_json::from_str(&response_text).with_context(|| {
                format!(
                    "Failed to parse AMFI response for ISIN: {identifier}. Response: '{response_text}'",
                )
            })?;

        debug!(
            "Successfully fetched price for ISIN {}: {:?}",
            identifier, amfi_response.nav
        );

        let current_price = amfi_response.nav;
        let currency = "INR".to_string();
        let short_name = amfi_response.name;

        let mut historical_prices = HashMap::new();

        if !amfi_response.historical_nav.is_empty() {
            let prices: Vec<_> = amfi_response
                .historical_nav
                .iter()
                .filter_map(|(date_str, price)| {
                    chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
                        .ok()
                        .map(|date| (date, *price))
                })
                .collect();

            if !prices.is_empty() {
                let current_nav_date =
                    chrono::NaiveDate::parse_from_str(&amfi_response.date, "%Y-%m-%d")
                        .unwrap_or_else(|e| {
                            debug!(
                                "Could not parse date from AMFI response for ISIN {}: '{}' ({}). Falling back to current date.",
                                identifier, amfi_response.date, e
                            );
                            chrono::Utc::now().date_naive()
                        });
                for period in [
                    HistoricalPeriod::OneDay,
                    HistoricalPeriod::FiveDays,
                    HistoricalPeriod::OneMonth,
                    HistoricalPeriod::OneYear,
                    HistoricalPeriod::ThreeYears,
                    HistoricalPeriod::FiveYears,
                    HistoricalPeriod::TenYears,
                ] {
                    let period_start_date = current_nav_date - period.to_duration();

                    if let Some((_date, price)) = prices
                        .iter()
                        .rev()
                        .find(|(date, _)| *date <= period_start_date)
                        && *price > 0.0
                    {
                        historical_prices.insert(period, *price);
                    }
                }
            }
        }

        let mut daily_prices: Vec<(chrono::NaiveDate, f64)> = amfi_response
            .historical_nav
            .into_iter()
            .map(|(date_str, price)| {
                chrono::NaiveDate::parse_from_str(&date_str, "%Y-%m-%d").map(|date| (date, price))
            })
            .filter_map(Result::ok)
            .collect();

        // Add current day data
        if let Ok(current_date) = chrono::NaiveDate::parse_from_str(&amfi_response.date, "%Y-%m-%d")
        {
            daily_prices.push((current_date, current_price));
        }

        // Sort by date and remove duplicates (keep last occurrence for same date)
        daily_prices.sort_by_key(|(date, _)| *date);
        daily_prices.dedup_by_key(|(date, _)| *date);

        let result = PriceResult {
            price: current_price,
            currency,
            historical_prices,
            daily_prices,
            short_name,
        };

        // Calculate TTL until next refresh at 7PM UTC
        let ttl_seconds = match seconds_until(19, 0) {
            Ok(ttl) => ttl,
            Err(e) => {
                warn!(
                    "Failed calculating 7PM UTC refresh TTL: {}. Using fallback 1 day",
                    e
                );
                24 * 60 * 60 // Fallback to 1 day
            }
        };

        // Cache with TTL aligned to 7PM UTC refresh schedule
        self.cache
            .put(
                identifier.as_bytes(),
                &serde_json::to_vec(&result).unwrap(),
                Some(Duration::from_secs(ttl_seconds)),
            )
            .await;

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::memory::MemoryCollection;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // Helper function to create a mock server for AMFI provider
    async fn create_amfi_mock_server(
        isin: &str,
        mock_response: &str,
        status_code: u16,
    ) -> MockServer {
        let mock_server = MockServer::start().await;
        let expected_path = format!("/nav/{isin}");

        Mock::given(method("GET"))
            .and(path(&expected_path))
            .respond_with(ResponseTemplate::new(status_code).set_body_string(mock_response))
            .mount(&mock_server)
            .await;
        mock_server
    }

    #[tokio::test]
    async fn test_successful_amfi_price_fetch() {
        let isin = "INF789F01XA0";
        let mock_response = r#"{"nav": 123.45, "date": "2024-01-01", "name": "My Fund"}"#;
        let mock_server = create_amfi_mock_server(isin, mock_response, 200).await;
        let cache = Arc::new(MemoryCollection::new());

        let provider = AmfiProvider::new_with_collection(&mock_server.uri(), cache);
        let result = provider.fetch_price(isin).await.unwrap();
        assert_eq!(result.short_name, Some("My Fund".to_string()));

        assert_eq!(result.price, 123.45);
        assert_eq!(result.currency, "INR");
        assert_eq!(result.short_name, Some("My Fund".to_string()));
    }

    #[tokio::test]
    async fn test_successful_amfi_price_fetch_with_full_historical_data() {
        let isin = "INF789F01XA0";
        let now = chrono::Utc::now().date_naive();
        let current_price = 150.0;

        let date_5y = (now - chrono::Duration::days(365 * 5 - 1))
            .format("%Y-%m-%d")
            .to_string();
        let price_5y = 100.0;
        let date_1y = (now - chrono::Duration::days(365))
            .format("%Y-%m-%d")
            .to_string();
        let price_1y = 120.0;
        let date_1m = (now - chrono::Duration::weeks(4) - chrono::Duration::days(2))
            .format("%Y-%m-%d")
            .to_string();
        let price_1m = 130.0;
        let date_5d = (now - chrono::Duration::days(5) - chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string();
        let price_5d = 140.0;

        let mock_response = format!(
            r#"{{"nav": {current_price}, "date": "{}", "name": "My Fund", "historical_nav": [["{date_5y}", {price_5y}], ["{date_1y}", {price_1y}], ["{date_1m}", {price_1m}], ["{date_5d}", {price_5d}]]}}"#,
            now.format("%Y-%m-%d"),
        );

        let mock_server = create_amfi_mock_server(isin, &mock_response, 200).await;
        let cache = Arc::new(MemoryCollection::new());
        let provider = AmfiProvider::new_with_collection(&mock_server.uri(), cache);
        let result = provider.fetch_price(isin).await.unwrap();

        assert_eq!(result.price, current_price);
        assert_eq!(result.currency, "INR");
        assert_eq!(result.short_name, Some("My Fund".to_string()));

        // 1d is added based on last price
        // 10y is not available as last data is < 5y
        // 5y is also ignored because we don't have a data point >= 5y
        assert_eq!(result.historical_prices.len(), 5);

        assert!(
            !result
                .historical_prices
                .contains_key(&HistoricalPeriod::TenYears)
        );
        assert!(
            !result
                .historical_prices
                .contains_key(&HistoricalPeriod::FiveYears)
        );

        // 3y uses the data <5y
        assert!(
            (result
                .historical_prices
                .get(&HistoricalPeriod::ThreeYears)
                .unwrap()
                - price_5y)
                .abs()
                < 0.001
        );

        assert!(
            (result
                .historical_prices
                .get(&HistoricalPeriod::OneYear)
                .unwrap()
                - price_1y)
                .abs()
                < 0.001
        );

        assert!(
            (result
                .historical_prices
                .get(&HistoricalPeriod::OneMonth)
                .unwrap()
                - price_1m)
                .abs()
                < 0.001
        );

        assert!(
            (result
                .historical_prices
                .get(&HistoricalPeriod::FiveDays)
                .unwrap()
                - price_5d)
                .abs()
                < 0.001
        );
    }

    #[tokio::test]
    async fn test_successful_amfi_price_fetch_with_partial_historical_data() {
        let isin = "INF789F01XA0";
        let now = chrono::Utc::now().date_naive();
        let current_price = 150.0;

        let date_1y = (now - chrono::Duration::days(365 + 10))
            .format("%Y-%m-%d")
            .to_string();
        let price_1y = 120.0;
        let date_1m = (now - chrono::Duration::weeks(4) - chrono::Duration::days(2))
            .format("%Y-%m-%d")
            .to_string();
        let price_1m = 130.0;

        let mock_response = format!(
            r#"{{"nav": {current_price}, "date": "{}", "name": "My Fund", "historical_nav": [["{date_1y}", {price_1y}], ["{date_1m}", {price_1m}]]}}"#,
            now.format("%Y-%m-%d"),
        );

        let mock_server = create_amfi_mock_server(isin, &mock_response, 200).await;
        let cache = Arc::new(MemoryCollection::new());
        let provider = AmfiProvider::new_with_collection(&mock_server.uri(), cache);
        let result = provider.fetch_price(isin).await.unwrap();

        // 1D, 5D will use the closest data >1 month old.
        // 1Y and 1M will resolve to their respective prices.
        // 10Y, 5Y, 3Y will not be available since we don't have the data points.
        assert_eq!(
            result.historical_prices.len(),
            4,
            "{:?}",
            result.historical_prices
        );

        assert!(
            (result
                .historical_prices
                .get(&HistoricalPeriod::OneMonth)
                .unwrap()
                - price_1m)
                .abs()
                < 0.001
        );

        assert!(
            (result
                .historical_prices
                .get(&HistoricalPeriod::OneYear)
                .unwrap()
                - price_1y)
                .abs()
                < 0.001
        );
        assert!(
            !result
                .historical_prices
                .contains_key(&HistoricalPeriod::ThreeYears)
        );
        assert!(
            !result
                .historical_prices
                .contains_key(&HistoricalPeriod::FiveYears)
        );
        assert!(
            !result
                .historical_prices
                .contains_key(&HistoricalPeriod::TenYears)
        );
    }

    #[tokio::test]
    async fn test_amfi_api_error_response() {
        let isin = "INF789F01XA0";
        let mock_server = create_amfi_mock_server(isin, "Server Error", 500).await;
        let cache = Arc::new(MemoryCollection::new());

        let provider = AmfiProvider::new_with_collection(&mock_server.uri(), cache);
        let result = provider.fetch_price(isin).await;

        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.starts_with(&format!("Failed to parse AMFI response for ISIN: {isin}")),);
    }

    #[tokio::test]
    async fn test_amfi_api_malformed_response() {
        let isin = "INF789F01XA0";
        let mock_response = r#"{ "not_nav": "abc" }"#; // Malformed JSON for AmfiResponse
        let mock_server = create_amfi_mock_server(isin, mock_response, 200).await;
        let cache = Arc::new(MemoryCollection::new());

        let provider = AmfiProvider::new_with_collection(&mock_server.uri(), cache);
        let result = provider.fetch_price(isin).await;

        assert!(result.is_err());
        let error_message = result.unwrap_err().to_string();
        assert!(error_message.contains("Failed to parse AMFI response"));
        assert!(error_message.contains(&format!("ISIN: {isin}")));
        assert!(error_message.contains("Response: '{ \"not_nav\": \"abc\" }'"));
    }

    #[tokio::test]
    async fn test_amfi_api_empty_response() {
        let isin = "INF789F01XA0";
        let mock_response = r#""#; // Empty response string
        let mock_server = create_amfi_mock_server(isin, mock_response, 200).await;
        let cache = Arc::new(MemoryCollection::new());

        let provider = AmfiProvider::new_with_collection(&mock_server.uri(), cache);
        let result = provider.fetch_price(isin).await;

        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            format!("Received empty response for ISIN: {isin}")
        );
    }
}
