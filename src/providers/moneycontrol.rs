use super::util::with_retry;
use crate::core::cache::{KeyValueCollection, Store};
use crate::core::{HistoricalPeriod, PriceProvider, PriceResult};
use crate::store::KeyValueStore;
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::NaiveDate;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::debug;

pub struct MoneyControlProvider {
    base_url: String,
    cache: Arc<dyn KeyValueCollection>,
}

impl MoneyControlProvider {
    pub fn new(base_url: &str, cache: Arc<KeyValueStore>) -> Self {
        let collection = cache.get_collection("moneycontrol", true, true).unwrap();
        MoneyControlProvider {
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
struct MoneyControlResponse {
    success: u8,
    data: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct NavEntry {
    #[serde(rename = "navDate")]
    nav_date: String,
    #[serde(rename = "navValue")]
    nav_value: f64,
}

#[async_trait]
impl PriceProvider for MoneyControlProvider {
    async fn fetch_price(&self, identifier: &str) -> Result<PriceResult> {
        if let Some(cached) = self.cache.get(identifier.as_bytes()).await {
            return Ok(serde_json::from_slice(&cached)?);
        }

        let url = format!(
            "{}/swiftapi/v1/mutualfunds/graph-nav?=&isin={}&dur=ALL&deviceType=W&responseType=json",
            self.base_url, identifier,
        );
        debug!("Requesting price data from {}", url);

        let client = reqwest::Client::builder().user_agent("xmf/1.0").build()?;
        let response = with_retry(|| async { client.get(&url).send().await }, 3, 500)
            .await
            .with_context(|| format!("Failed to send request for ISIN: {identifier}"))?;

        let response_text = response
            .text()
            .await
            .with_context(|| format!("Failed to get response text for ISIN: {identifier}"))?;

        if response_text.trim().is_empty() {
            return Err(anyhow::anyhow!(
                "Received empty response for ISIN: {}",
                identifier
            ));
        }

        let mc_response: MoneyControlResponse =
            serde_json::from_str(&response_text).with_context(|| {
                format!(
                    "Failed to parse MoneyControl response for ISIN: {identifier}. Response: '{response_text}'",
                )
            })?;

        if mc_response.success != 1 {
            return Err(anyhow::anyhow!(
                "MoneyControl API returned success={} for ISIN: {}",
                mc_response.success,
                identifier
            ));
        }

        // The data field contains per-ISIN nav arrays plus an "availPeriod" key.
        // Extract the NAV entries for the requested ISIN, or fall back to the
        // first array-typed value in the map.
        let entries: Vec<NavEntry> = if let Some(arr) = mc_response.data.get(identifier) {
            serde_json::from_value(arr.clone()).unwrap_or_default()
        } else {
            mc_response
                .data
                .as_object()
                .and_then(|m| m.values().find(|v| v.is_array()))
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default()
        };

        if entries.is_empty() {
            return Err(anyhow::anyhow!("Empty NAV data for ISIN: {}", identifier));
        }

        let mut daily_prices: Vec<(NaiveDate, f64)> = entries
            .iter()
            .filter_map(|entry| {
                NaiveDate::parse_from_str(&entry.nav_date, "%Y-%m-%d")
                    .ok()
                    .map(|date| (date, entry.nav_value))
            })
            .collect();

        daily_prices.sort_by_key(|(date, _)| *date);
        daily_prices.dedup_by_key(|(date, _)| *date);

        let current_price = daily_prices.last().map(|(_, p)| *p).unwrap_or(0.0);
        let currency = "INR".to_string();

        let mut historical_prices = HashMap::new();
        if let Some((current_date, _)) = daily_prices.last() {
            for period in [
                HistoricalPeriod::OneDay,
                HistoricalPeriod::FiveDays,
                HistoricalPeriod::OneMonth,
                HistoricalPeriod::OneYear,
                HistoricalPeriod::ThreeYears,
                HistoricalPeriod::FiveYears,
                HistoricalPeriod::TenYears,
            ] {
                let target_date = *current_date - period.to_duration();
                if let Some((_, price)) = daily_prices
                    .iter()
                    .rev()
                    .find(|(date, _)| *date <= target_date)
                    && *price > 0.0
                {
                    historical_prices.insert(period, *price);
                }
            }
        }

        let short_name = None;

        let result = PriceResult {
            price: current_price,
            currency,
            historical_prices,
            daily_prices,
            short_name,
        };

        let ttl_seconds = crate::providers::util::seconds_until(19, 0).unwrap_or(24 * 60 * 60);

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

    async fn create_mock_server(mock_response: &str) -> MockServer {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/swiftapi/v1/mutualfunds/graph-nav"))
            .respond_with(ResponseTemplate::new(200).set_body_string(mock_response))
            .mount(&mock_server)
            .await;

        mock_server
    }

    #[tokio::test]
    async fn test_successful_fetch() {
        let isin = "INF789F01VB2";
        let mock_response = r#"{"success":1,"data":{"INF789F01VB2":[{"navDate":"2026-08-27","navValue":183.86,"navValueAdjusted":183.86,"changeNAV":-0.81,"changePercentNAV":-0.44},{"navDate":"2026-08-28","navValue":184.4,"navValueAdjusted":184.4,"changeNAV":0.54,"changePercentNAV":0.29}],"availPeriod":["1M","3M","ALL"]}}"#;
        let mock_server = create_mock_server(mock_response).await;
        let cache = Arc::new(MemoryCollection::new());
        let provider = MoneyControlProvider::new_with_collection(&mock_server.uri(), cache);

        let result = provider.fetch_price(isin).await.unwrap();
        assert_eq!(result.price, 184.4);
        assert_eq!(result.currency, "INR");
        assert_eq!(result.daily_prices.len(), 2);
        assert_eq!(result.daily_prices[0].1, 183.86);
        assert_eq!(result.daily_prices[1].1, 184.4);
    }

    #[tokio::test]
    async fn test_historical_prices_computed() {
        let isin = "INF789F01VB2";
        let mock_response = r#"{"success":1,"data":{"INF789F01VB2":[{"navDate":"2026-08-20","navValue":184.75,"navValueAdjusted":184.75,"changeNAV":1.25,"changePercentNAV":0.68},{"navDate":"2026-08-21","navValue":184.92,"navValueAdjusted":184.92,"changeNAV":0.17,"changePercentNAV":0.09},{"navDate":"2026-08-24","navValue":185.11,"navValueAdjusted":185.11,"changeNAV":0.19,"changePercentNAV":0.1},{"navDate":"2026-08-25","navValue":185.43,"navValueAdjusted":185.43,"changeNAV":0.32,"changePercentNAV":0.17},{"navDate":"2026-08-26","navValue":184.67,"navValueAdjusted":184.67,"changeNAV":-0.76,"changePercentNAV":-0.41},{"navDate":"2026-08-27","navValue":183.86,"navValueAdjusted":183.86,"changeNAV":-0.81,"changePercentNAV":-0.44},{"navDate":"2026-08-28","navValue":184.4,"navValueAdjusted":184.4,"changeNAV":0.54,"changePercentNAV":0.29}],"availPeriod":["1M","3M","ALL"]}}"#;
        let mock_server = create_mock_server(mock_response).await;
        let cache = Arc::new(MemoryCollection::new());
        let provider = MoneyControlProvider::new_with_collection(&mock_server.uri(), cache);

        let result = provider.fetch_price(isin).await.unwrap();
        // 1D: closest date <= Aug 28 - 1d = Aug 27 -> 183.86
        assert_eq!(
            result.historical_prices.get(&HistoricalPeriod::OneDay),
            Some(&183.86)
        );
        // 5D: closest date <= Aug 28 - 5d = Aug 23 -> Aug 21 (184.92)
        assert_eq!(
            result.historical_prices.get(&HistoricalPeriod::FiveDays),
            Some(&184.92)
        );
    }

    #[tokio::test]
    async fn test_empty_data_error() {
        let isin = "INVALID";
        let mock_response = r#"{"success":1,"data":{}}"#;
        let mock_server = create_mock_server(mock_response).await;
        let cache = Arc::new(MemoryCollection::new());
        let provider = MoneyControlProvider::new_with_collection(&mock_server.uri(), cache);

        let result = provider.fetch_price(isin).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cache_hit() {
        let isin = "INF789F01VB2";
        let mock_response = r#"{"success":1,"data":{"INF789F01VB2":[{"navDate":"2026-08-28","navValue":184.4,"navValueAdjusted":184.4,"changeNAV":0.54,"changePercentNAV":0.29}],"availPeriod":["1M","3M","ALL"]}}"#;
        let mock_server = create_mock_server(mock_response).await;
        let cache = Arc::new(MemoryCollection::new());
        let provider = MoneyControlProvider::new_with_collection(&mock_server.uri(), cache);

        // First call hits network
        provider.fetch_price(isin).await.unwrap();
        // Second call hits cache
        provider.fetch_price(isin).await.unwrap();
    }
}
