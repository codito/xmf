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
    // Add other fields if needed for debugging or future use, e.g.:
    // ISIN: String,
    // name: String,
    // date: String,
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

        let historical = HashMap::new();

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
        mock_response: &'static str,
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
