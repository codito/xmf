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
                let periods = [
                    (HistoricalPeriod::OneWeek, chrono::Duration::weeks(1)),
                    (HistoricalPeriod::OneMonth, chrono::Duration::weeks(4)),
                    (HistoricalPeriod::OneYear, chrono::Duration::days(365)),
                    (HistoricalPeriod::ThreeYears, chrono::Duration::days(365 * 3)),
                    (HistoricalPeriod::FiveYears, chrono::Duration::days(365 * 5)),
                ];

                let now = chrono::Utc::now().date_naive();

                for (period, duration) in periods {
                    let period_start_date = now - duration;

                    if let Some((_date, price)) =
                        prices.iter().find(|(date, _)| *date >= period_start_date)
                    {
                        historical.insert(period, *price);
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

        let date_5y = (now - chrono::Duration::days(365 * 5 - 10))
            .format("%Y-%m-%d")
            .to_string();
        let price_5y = 100.0;
        let date_3y = (now - chrono::Duration::days(365 * 3 - 10))
            .format("%Y-%m-%d")
            .to_string();
        let price_3y = 110.0;
        let date_1y = (now - chrono::Duration::days(365 - 10))
            .format("%Y-%m-%d")
            .to_string();
        let price_1y = 120.0;
        let date_1m = (now - chrono::Duration::weeks(4) + chrono::Duration::days(2))
            .format("%Y-%m-%d")
            .to_string();
        let price_1m = 130.0;
        let date_1w = (now - chrono::Duration::weeks(1) + chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string();
        let price_1w = 140.0;

        let mock_response = format!(
            r#"{{"nav": 150.0, "historical_nav": [["{}", {}], ["{}", {}], ["{}", {}], ["{}", {}], ["{}", {}]]}}"#,
            date_5y, price_5y, date_3y, price_3y, date_1y, price_1y, date_1m, price_1m, date_1w, price_1w
        );

        let mock_server = create_amfi_mock_server(isin, &mock_response, 200).await;
        let provider = AmfiProvider::new(&mock_server.uri());
        let result = provider.fetch_price(isin).await.unwrap();

        assert_eq!(result.price, 150.0);
        assert_eq!(result.currency, "INR");
        assert_eq!(result.historical.len(), 5);
        assert_eq!(
            *result.historical.get(&HistoricalPeriod::FiveYears).unwrap(),
            price_5y
        );
        assert_eq!(
            *result.historical.get(&HistoricalPeriod::ThreeYears).unwrap(),
            price_3y
        );
        assert_eq!(
            *result.historical.get(&HistoricalPeriod::OneYear).unwrap(),
            price_1y
        );
        assert_eq!(
            *result.historical.get(&HistoricalPeriod::OneMonth).unwrap(),
            price_1m
        );
        assert_eq!(
            *result.historical.get(&HistoricalPeriod::OneWeek).unwrap(),
            price_1w
        );
    }

    #[tokio::test]
    async fn test_successful_amfi_price_fetch_with_partial_historical_data() {
        let isin = "INF789F01XA0";
        let now = chrono::Utc::now().date_naive();
        let date_1y = (now - chrono::Duration::days(365 - 10))
            .format("%Y-%m-%d")
            .to_string();
        let price_1y = 120.0;
        let date_1m = (now - chrono::Duration::weeks(4) + chrono::Duration::days(2))
            .format("%Y-%m-%d")
            .to_string();
        let price_1m = 130.0;

        let mock_response = format!(
            r#"{{"nav": 150.0, "historical_nav": [["{}", {}], ["{}", {}]]}}"#,
            date_1y, price_1y, date_1m, price_1m
        );

        let mock_server = create_amfi_mock_server(isin, &mock_response, 200).await;
        let provider = AmfiProvider::new(&mock_server.uri());
        let result = provider.fetch_price(isin).await.unwrap();

        // OneWeek is missing as the closest data is >1 month old
        // 5Y, 3Y, 1Y will all resolve to the 1Y price
        assert_eq!(result.historical.len(), 4);
        assert!(result.historical.get(&HistoricalPeriod::OneWeek).is_none());
        assert_eq!(
            *result.historical.get(&HistoricalPeriod::OneMonth).unwrap(),
            price_1m
        );
        assert_eq!(
            *result.historical.get(&HistoricalPeriod::OneYear).unwrap(),
            price_1y
        );
        assert_eq!(
            *result.historical.get(&HistoricalPeriod::ThreeYears).unwrap(),
            price_1y
        );
        assert_eq!(
            *result.historical.get(&HistoricalPeriod::FiveYears).unwrap(),
            price_1y
        );
    }

    #[tokio::test]
    async fn test_successful_amfi_price_fetch_with_empty_historical_data() {
        let isin = "INF789F01XA0";
        let mock_response = r#"{"nav": 123.45, "historical_nav": []}"#;
        let mock_server = create_amfi_mock_server(isin, mock_response, 200).await;

        let provider = AmfiProvider::new(&mock_server.uri());
        let result = provider.fetch_price(isin).await.unwrap();

        assert!(result.historical.is_empty());
    }

    #[tokio::test]
    async fn test_successful_amfi_price_fetch_with_malformed_historical_data() {
        let isin = "INF789F01XA0";
        let now = chrono::Utc::now().date_naive();
        let date_1w = (now - chrono::Duration::weeks(1) + chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string();
        let price_1w = 140.0;
        let mock_response = format!(
            r#"{{"nav": 150.0, "historical_nav": [["bad-date", 100.0], ["{}", {}]]}}"#,
            date_1w, price_1w
        );

        let mock_server = create_amfi_mock_server(isin, &mock_response, 200).await;
        let provider = AmfiProvider::new(&mock_server.uri());
        let result = provider.fetch_price(isin).await.unwrap();

        // The malformed date is ignored, and the valid 1-week data is used for all periods.
        assert_eq!(result.historical.len(), 5);
        assert_eq!(
            *result.historical.get(&HistoricalPeriod::OneWeek).unwrap(),
            price_1w
        );
        assert_eq!(
            *result.historical.get(&HistoricalPeriod::OneMonth).unwrap(),
            price_1w
        );
        assert_eq!(
            *result.historical.get(&HistoricalPeriod::OneYear).unwrap(),
            price_1w
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
