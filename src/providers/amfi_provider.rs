use crate::price_provider::{HistoricalPeriod, PriceProvider, PriceResult};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use chrono;
use serde::Deserialize;
use std::collections::HashMap;
use tracing::debug;

pub struct AmfiProvider {
    base_url: String,
}

impl AmfiProvider {
    pub fn new(base_url: &str) -> Self {
        AmfiProvider {
            base_url: base_url.to_string(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct AmfiResponse {
    nav: f64,
    #[serde(default)]
    historical_nav: Vec<(String, f64)>,
}

#[async_trait]
impl PriceProvider for AmfiProvider {
    async fn fetch_price(&self, identifier: &str) -> Result<PriceResult> {
        let url = format!("{}/nav/{}", self.base_url, identifier);
        debug!("Requesting price data from {}", url);

        let client = reqwest::Client::builder().user_agent("xmf/1.0").build()?;
        let response = client
            .get(&url)
            .send()
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

        let mut historical = HashMap::new();

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
                let now = chrono::Utc::now().date_naive();
                for period in [
                    HistoricalPeriod::OneDay,
                    HistoricalPeriod::FiveDays,
                    HistoricalPeriod::OneMonth,
                    HistoricalPeriod::OneYear,
                    HistoricalPeriod::ThreeYears,
                    HistoricalPeriod::FiveYears,
                    HistoricalPeriod::TenYears,
                ] {
                    let period_start_date = now - period.to_duration();

                    if let Some((_date, price)) =
                        prices.iter().find(|(date, _)| *date >= period_start_date)
                    {
                        if *price > 0.0 {
                            let change = ((current_price - price) / price) * 100.0;
                            historical.insert(period, change);
                        }
                    }
                }
            }
        }

        Ok(PriceResult {
            price: current_price,
            currency,
            historical,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let mock_response = r#"{"nav": 123.45}"#;
        let mock_server = create_amfi_mock_server(isin, mock_response, 200).await;

        let provider = AmfiProvider::new(&mock_server.uri());
        let result = provider.fetch_price(isin).await.unwrap();

        assert_eq!(result.price, 123.45);
        assert_eq!(result.currency, "INR");
    }

    #[tokio::test]
    async fn test_successful_amfi_price_fetch_with_full_historical_data() {
        let isin = "INF789F01XA0";
        let now = chrono::Utc::now().date_naive();
        let current_price = 150.0;

        let date_5y = (now - chrono::Duration::days(365 * 5 - 10))
            .format("%Y-%m-%d")
            .to_string();
        let price_5y = 100.0;
        let date_1y = (now - chrono::Duration::days(365 - 10))
            .format("%Y-%m-%d")
            .to_string();
        let price_1y = 120.0;
        let date_1m = (now - chrono::Duration::weeks(4) + chrono::Duration::days(2))
            .format("%Y-%m-%d")
            .to_string();
        let price_1m = 130.0;
        let date_5d = (now - chrono::Duration::days(5) + chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string();
        let price_5d = 140.0;

        let mock_response = format!(
            r#"{{"nav": {current_price}, "historical_nav": [["{date_5y}", {price_5y}], ["{date_1y}", {price_1y}], ["{date_1m}", {price_1m}], ["{date_5d}", {price_5d}]]}}"#,
        );

        let mock_server = create_amfi_mock_server(isin, &mock_response, 200).await;
        let provider = AmfiProvider::new(&mock_server.uri());
        let result = provider.fetch_price(isin).await.unwrap();

        assert_eq!(result.price, current_price);
        assert_eq!(result.currency, "INR");
        assert_eq!(result.historical.len(), 6);

        let expected_change_5y = ((current_price - price_5y) / price_5y) * 100.0;
        assert!(
            (result.historical.get(&HistoricalPeriod::FiveYears).unwrap() - expected_change_5y)
                .abs()
                < 0.001
        );
        assert!(
            (result.historical.get(&HistoricalPeriod::TenYears).unwrap() - expected_change_5y)
                .abs()
                < 0.001
        );

        let expected_change_1y = ((current_price - price_1y) / price_1y) * 100.0;
        assert!(
            (result.historical.get(&HistoricalPeriod::OneYear).unwrap() - expected_change_1y).abs()
                < 0.001
        );
        assert!(
            (result
                .historical
                .get(&HistoricalPeriod::ThreeYears)
                .unwrap()
                - expected_change_1y)
                .abs()
                < 0.001
        );

        let expected_change_1m = ((current_price - price_1m) / price_1m) * 100.0;
        assert!(
            (result
                .historical
                .get(&HistoricalPeriod::OneMonth)
                .unwrap()
                - expected_change_1m)
                .abs()
                < 0.001
        );

        let expected_change_5d = ((current_price - price_5d) / price_5d) * 100.0;
        assert!(
            (result.historical.get(&HistoricalPeriod::FiveDays).unwrap() - expected_change_5d)
                .abs()
                < 0.001
        );
    }

    #[tokio::test]
    async fn test_successful_amfi_price_fetch_with_partial_historical_data() {
        let isin = "INF789F01XA0";
        let now = chrono::Utc::now().date_naive();
        let current_price = 150.0;

        let date_1y = (now - chrono::Duration::days(365 - 10))
            .format("%Y-%m-%d")
            .to_string();
        let price_1y = 120.0;
        let date_1m = (now - chrono::Duration::weeks(4) + chrono::Duration::days(2))
            .format("%Y-%m-%d")
            .to_string();
        let price_1m = 130.0;

        let mock_response = format!(
            r#"{{"nav": {current_price}, "historical_nav": [["{date_1y}", {price_1y}], ["{date_1m}", {price_1m}]]}}"#,
        );

        let mock_server = create_amfi_mock_server(isin, &mock_response, 200).await;
        let provider = AmfiProvider::new(&mock_server.uri());
        let result = provider.fetch_price(isin).await.unwrap();

        // 1D, 5D are missing as the closest data is >1 month old
        // 10Y, 5Y, 3Y will resolve to the 1Y price. 1Y and 1M will resolve to their respective prices.
        assert_eq!(result.historical.len(), 5);

        let expected_change_1m = ((current_price - price_1m) / price_1m) * 100.0;
        assert!(
            (result.historical.get(&HistoricalPeriod::OneMonth).unwrap() - expected_change_1m)
                .abs()
                < 0.001
        );

        let expected_change_1y = ((current_price - price_1y) / price_1y) * 100.0;
        assert!(
            (result.historical.get(&HistoricalPeriod::OneYear).unwrap() - expected_change_1y).abs()
                < 0.001
        );
        assert!(
            (result
                .historical
                .get(&HistoricalPeriod::ThreeYears)
                .unwrap()
                - expected_change_1y)
                .abs()
                < 0.001
        );
        assert!(
            (result.historical.get(&HistoricalPeriod::FiveYears).unwrap() - expected_change_1y)
                .abs()
                < 0.001
        );
        assert!(
            (result.historical.get(&HistoricalPeriod::TenYears).unwrap() - expected_change_1y)
                .abs()
                < 0.001
        );
    }

    #[tokio::test]
    async fn test_amfi_api_error_response() {
        let isin = "INF789F01XA0";
        let mock_server = create_amfi_mock_server(isin, "Server Error", 500).await;

        let provider = AmfiProvider::new(&mock_server.uri());
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

        let provider = AmfiProvider::new(&mock_server.uri());
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

        let provider = AmfiProvider::new(&mock_server.uri());
        let result = provider.fetch_price(isin).await;

        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            format!("Received empty response for ISIN: {isin}")
        );
    }
}
