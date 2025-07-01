use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono;
use serde::Deserialize;
use std::collections::HashMap;
use tracing::{debug, instrument};

use crate::currency_provider::CurrencyRateProvider;
use crate::price_provider::{HistoricalPeriod, PriceProvider, PriceResult};

// YahooFinanceProvider implementation for PriceProvider
pub struct YahooFinanceProvider {
    base_url: String,
}

impl YahooFinanceProvider {
    pub fn new(base_url: &str) -> Self {
        YahooFinanceProvider {
            base_url: base_url.to_string(),
        }
    }
}

#[derive(Deserialize, Debug)]
struct YahooPriceResponse {
    chart: PriceChartResult,
}

#[derive(Deserialize, Debug)]
struct PriceChartResult {
    result: Vec<PriceChartItem>,
}

#[derive(Deserialize, Debug)]
struct Indicators {
    quote: Vec<Quote>,
}

#[derive(Deserialize, Debug)]
struct Quote {
    close: Option<Vec<Option<f64>>>,
}

#[derive(Deserialize, Debug)]
struct PriceChartItem {
    meta: PriceChartMeta,
    timestamp: Option<Vec<i64>>,
    indicators: Option<Indicators>,
}

#[derive(Deserialize, Debug)]
struct PriceChartMeta {
    #[serde(alias = "regularMarketPrice")]
    regular_market_price: f64,
    currency: String,
}

#[async_trait]
impl PriceProvider for YahooFinanceProvider {
    #[instrument(
        name = "YahooPriceFetch",
        skip(self),
        fields(symbol = %symbol)
    )]
    async fn fetch_price(&self, symbol: &str) -> Result<PriceResult> {
        let url = format!("{}/v8/finance/chart/{}?interval=1d&range=10y", self.base_url, symbol);
        debug!("Requesting price data from {}", url);

        let client = reqwest::Client::builder().user_agent("xmf/1.0").build()?;
        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| anyhow!("Request error: {} for symbol: {} URL: {}", e, symbol, url))?;

        debug!(response = ?response, "Received Yahoo response");

        let data = response.json::<YahooPriceResponse>().await?;
        let item = data.chart.result
            .first()
            .ok_or_else(|| anyhow!("No price data found for symbol: {}", symbol))?;

        let current_price = item.meta.regular_market_price;
        let currency = item.meta.currency.clone();

        let mut historical = HashMap::new();

        // Extract timestamps and prices if available
        if let (Some(timestamps), Some(closes)) = (
            item.timestamp.as_ref(),
            item.indicators
                .as_ref()
                .and_then(|inds| inds.quote.get(0))
                .and_then(|q| q.close.as_ref()),
        ) {
            // Create vector of (timestamp, price) for non-empty prices
            let prices: Vec<_> = timestamps
                .iter()
                .zip(closes.iter())
                .filter_map(|(ts, opt_price)| opt_price.map(|price| (*ts, price)))
                .collect();

            let periods = [
                (HistoricalPeriod::OneWeek, chrono::Duration::weeks(1)),
                (HistoricalPeriod::OneMonth, chrono::Duration::weeks(4)),
                (HistoricalPeriod::OneYear, chrono::Duration::days(365)),
                (HistoricalPeriod::ThreeYears, chrono::Duration::days(365 * 3)),
                (HistoricalPeriod::FiveYears, chrono::Duration::days(365 * 5)),
            ];

            for (period, duration) in periods {
                let period_start = (chrono::Utc::now() - duration).timestamp();
                // Find the first price at or after period_start
                if let Some(price) = prices
                    .iter()
                    .find(|(ts, _)| *ts >= period_start)
                    .map(|(_, price)| *price)
                {
                    historical.insert(period, price);
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

// YahooCurrencyProvider implementation for CurrencyRateProvider
pub struct YahooCurrencyProvider {
    base_url: String,
}

impl YahooCurrencyProvider {
    pub fn new(base_url: &str) -> Self {
        YahooCurrencyProvider {
            base_url: base_url.to_string(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct YahooCurrencyResponse {
    chart: CurrencyChartResult,
}

#[derive(Debug, Deserialize)]
struct CurrencyChartResult {
    result: Vec<CurrencyChartItem>,
}

#[derive(Debug, Deserialize)]
struct CurrencyChartItem {
    meta: CurrencyChartMeta,
}

#[derive(Debug, Deserialize)]
struct CurrencyChartMeta {
    #[serde(alias = "regularMarketPrice")]
    regular_market_price: f64,
}

#[async_trait]
impl CurrencyRateProvider for YahooCurrencyProvider {
    async fn get_rate(&self, from: &str, to: &str) -> Result<f64> {
        let symbol = format!("{from}{to}=X");
        let endpoint = format!("/v8/finance/chart/{symbol}");
        let url = format!("{}{}", self.base_url, endpoint);
        debug!("Requesting currency rate from {}", url);

        let client = reqwest::Client::builder().user_agent("xmf/1.0").build()?;

        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| anyhow!("Request error: {} for currency pair: {}", e, symbol))?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "HTTP error: {} for currency pair: {}",
                response.status(),
                symbol
            ));
        }

        let text = response.text().await?;

        let data: YahooCurrencyResponse = serde_json::from_str(&text)
            .map_err(|e| anyhow!("Failed to parse JSON response for {}: {}", symbol, e))?;

        let item = data
            .chart
            .result
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("No rate data found for currency pair: {}", symbol))?;

        Ok(item.meta.regular_market_price)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // Tests for YahooFinanceProvider (PriceProvider)
    pub async fn create_mock_server(
        symbol: &str,
        mock_response: &str,
    ) -> wiremock::MockServer {
        let mock_server = wiremock::MockServer::start().await;
        let request_path = format!("/v8/finance/chart/{symbol}");

        Mock::given(method("GET"))
            .and(path(request_path))
            .respond_with(ResponseTemplate::new(200).set_body_string(mock_response))
            .mount(&mock_server)
            .await;

        mock_server
    }

    #[tokio::test]
    async fn test_successful_price_fetch() {
        let mock_response = r#"{
            "chart": {
                "result": [{
                    "meta": {
                        "regularMarketPrice": 150.65,
                        "currency": "USD"
                    }
                }]
            }
        }"#;

        let mock_server = create_mock_server("AAPL", mock_response).await;

        let provider = YahooFinanceProvider::new(&mock_server.uri());
        let result = provider.fetch_price("AAPL").await.unwrap();
        assert_eq!(result.price, 150.65);
        assert_eq!(result.currency, "USD");
        assert!(result.historical.is_empty());
    }

    #[tokio::test]
    async fn test_successful_price_fetch_with_historical_data() {
        let now = chrono::Utc::now();
        let ts_5y = (now - chrono::Duration::days(365 * 5 - 10)).timestamp();
        let p_5y = 100.0;
        let ts_3y = (now - chrono::Duration::days(365 * 3 - 10)).timestamp();
        let p_3y = 110.0;
        let ts_1y = (now - chrono::Duration::days(365 - 10)).timestamp();
        let p_1y = 120.0;
        let ts_1m = (now - chrono::Duration::weeks(4) + chrono::Duration::days(2)).timestamp();
        let p_1m = 130.0;
        let ts_1w = (now - chrono::Duration::weeks(1) + chrono::Duration::days(2)).timestamp();
        let p_1w = 140.0;

        let mock_response = format!(
            r#"{{
                "chart": {{
                    "result": [{{
                        "meta": {{
                            "regularMarketPrice": 150.65,
                            "currency": "USD"
                        }},
                        "timestamp": [{}, {}, {}, {}, {}],
                        "indicators": {{
                            "quote": [{{
                                "close": [{}, {}, {}, {}, {}]
                            }}]
                        }}
                    }}]
                }}
            }}"#,
            ts_5y, ts_3y, ts_1y, ts_1m, ts_1w, p_5y, p_3y, p_1y, p_1m, p_1w
        );

        let mock_server = create_mock_server("AAPL", &mock_response).await;

        let provider = YahooFinanceProvider::new(&mock_server.uri());
        let result = provider.fetch_price("AAPL").await.unwrap();

        assert_eq!(result.price, 150.65);
        assert_eq!(result.currency, "USD");
        assert_eq!(result.historical.len(), 5);
        assert_eq!(
            result.historical.get(&HistoricalPeriod::FiveYears),
            Some(&p_5y)
        );
        assert_eq!(
            result.historical.get(&HistoricalPeriod::ThreeYears),
            Some(&p_3y)
        );
        assert_eq!(result.historical.get(&HistoricalPeriod::OneYear), Some(&p_1y));
        assert_eq!(result.historical.get(&HistoricalPeriod::OneMonth), Some(&p_1m));
        assert_eq!(result.historical.get(&HistoricalPeriod::OneWeek), Some(&p_1w));
    }

    #[tokio::test]
    async fn test_no_price_result_data() {
        let mock_response = r#"{"chart": {"result": []}}"#;
        let mock_server = create_mock_server("INVALID", mock_response).await;

        let provider = YahooFinanceProvider::new(&mock_server.uri());
        let result = provider.fetch_price("INVALID").await;
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "No price data found for symbol: INVALID"
        );
    }

    // Tests for YahooCurrencyProvider (CurrencyRateProvider)
    #[tokio::test]
    async fn test_successful_rate_fetch() {
        let mock_server = MockServer::start().await;
        let provider = YahooCurrencyProvider::new(&mock_server.uri());

        let mock_response = r#"{
            "chart": {
                "result": [
                    {
                        "meta": {
                            "regularMarketPrice": 1.2345
                        }
                    }
                ]
            }
        }"#;

        let expected_endpoint = "/v8/finance/chart/USDEUR=X";
        Mock::given(method("GET"))
            .and(path(expected_endpoint))
            .respond_with(ResponseTemplate::new(200).set_body_string(mock_response))
            .mount(&mock_server)
            .await;

        let rate = provider
            .get_rate("USD", "EUR")
            .await
            .expect("Failed to get rate");
        assert_eq!(rate, 1.2345);
    }

    #[tokio::test]
    async fn test_no_currency_rate_found() {
        let mock_server = MockServer::start().await;
        let provider = YahooCurrencyProvider::new(&mock_server.uri());

        let mock_response = r#"{
            "chart": {
                "result": []
            }
        }"#;

        let expected_endpoint = "/v8/finance/chart/USDEUR=X";
        Mock::given(method("GET"))
            .and(path(expected_endpoint))
            .respond_with(ResponseTemplate::new(200).set_body_string(mock_response))
            .mount(&mock_server)
            .await;

        let result = provider.get_rate("USD", "EUR").await;
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "No rate data found for currency pair: USDEUR=X"
        );
    }

    #[tokio::test]
    async fn test_yahoo_currency_api_error_response() {
        let mock_server = MockServer::start().await;
        let provider = YahooCurrencyProvider::new(&mock_server.uri());

        let expected_endpoint = "/v8/finance/chart/USDEUR=X";
        Mock::given(method("GET"))
            .and(path(expected_endpoint))
            .respond_with(ResponseTemplate::new(500)) // Simulate a server error
            .mount(&mock_server)
            .await;

        let result = provider.get_rate("USD", "EUR").await;
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "HTTP error: 500 Internal Server Error for currency pair: USDEUR=X"
        );
    }

    #[tokio::test]
    async fn test_yahoo_currency_api_malformed_response() {
        let mock_server = MockServer::start().await;
        let provider = YahooCurrencyProvider::new(&mock_server.uri());

        let mock_response = r#"{
            "chart": {
                "results": []
            }
        }"#; // "results" instead of "result"

        let expected_endpoint = "/v8/finance/chart/USDEUR=X";
        Mock::given(method("GET"))
            .and(path(expected_endpoint))
            .respond_with(ResponseTemplate::new(200).set_body_string(mock_response))
            .mount(&mock_server)
            .await;

        let result = provider.get_rate("USD", "EUR").await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Failed to parse JSON response for USDEUR=X")
        );
    }
}
