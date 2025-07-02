use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, instrument};

use crate::cache::Cache;
use crate::currency_provider::CurrencyRateProvider;
use crate::price_provider::{HistoricalPeriod, PriceProvider, PriceResult};

fn find_closest_price(target_ts: i64, timestamps: &[i64], prices: &[Option<f64>]) -> Option<f64> {
    timestamps
        .iter()
        .position(|ts| *ts >= target_ts)
        .and_then(|index| prices.get(index).and_then(|p| *p))
}

fn extract_historical_prices(
    chart_item: &PriceChartItem,
    current_price: f64,
) -> HashMap<HistoricalPeriod, f64> {
    let mut historical_changes = HashMap::new();

    if let (Some(timestamps), Some(closes)) = (
        chart_item.timestamp.as_ref(),
        chart_item
            .indicators
            .as_ref()
            .and_then(|inds| inds.quote.first())
            .and_then(|q| q.close.as_ref()),
    ) {
        let reference_date = match timestamps
            .last()
            .and_then(|ts| Utc.timestamp_opt(*ts, 0).single())
        {
            Some(dt) => dt,
            None => return historical_changes,
        };

        for period in [
            HistoricalPeriod::OneDay,
            HistoricalPeriod::FiveDays,
            HistoricalPeriod::OneMonth,
            HistoricalPeriod::OneYear,
            HistoricalPeriod::ThreeYears,
            HistoricalPeriod::FiveYears,
            HistoricalPeriod::TenYears,
        ] {
            if period == HistoricalPeriod::OneDay {
                if closes.len() >= 2 {
                    let price = closes[closes.len() - 2].unwrap();
                    if price > 0.0 {
                        let change = ((current_price - price) / price) * 100.0;
                        historical_changes.insert(period, change);
                    }
                }
                continue;
            }

            // Other periods
            let target_date = reference_date - period.to_duration();
            if let Some(price) = find_closest_price(target_date.timestamp(), timestamps, closes) {
                if price > 0.0 {
                    let change = ((current_price - price) / price) * 100.0;
                    historical_changes.insert(period, change);
                }
            }
        }
    } else if let Some(prev_close) = chart_item.meta.previous_close {
        // Handle case where we only have meta data (no historical bars)
        if prev_close > 0.0 {
            let change = ((current_price - prev_close) / prev_close) * 100.0;
            historical_changes.insert(HistoricalPeriod::OneDay, change);
        }
    }

    historical_changes
}

// YahooFinanceProvider implementation for PriceProvider
pub struct YahooFinanceProvider {
    base_url: String,
    cache: Arc<Cache<String, PriceResult>>,
}

impl YahooFinanceProvider {
    pub fn new(base_url: &str, cache: Arc<Cache<String, PriceResult>>) -> Self {
        YahooFinanceProvider {
            base_url: base_url.to_string(),
            cache,
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
    #[serde(alias = "chartPreviousClose")]
    previous_close: Option<f64>,
}

#[async_trait]
impl PriceProvider for YahooFinanceProvider {
    #[instrument(
        name = "YahooPriceFetch",
        skip(self),
        fields(symbol = %symbol)
    )]
    async fn fetch_price(&self, symbol: &str) -> Result<PriceResult> {
        if let Some(cached) = self.cache.get(&symbol.to_string()).await {
            return Ok(cached);
        }

        let url = format!(
            "{}/v8/finance/chart/{}?interval=1d&range=10y",
            self.base_url, symbol
        );
        debug!("Requesting price data from {}", url);

        let client = reqwest::Client::builder().user_agent("xmf/1.0").build()?;
        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| anyhow!("Request error: {} for symbol: {} URL: {}", e, symbol, url))?;

        debug!(response = ?response, "Received Yahoo response");

        let data = response.json::<YahooPriceResponse>().await?;
        let item = data
            .chart
            .result
            .first()
            .ok_or_else(|| anyhow!("No price data found for symbol: {}", symbol))?;

        let current_price = item.meta.regular_market_price;
        let currency = item.meta.currency.clone();

        let historical = extract_historical_prices(item, current_price);

        let result = PriceResult {
            price: current_price,
            currency,
            historical,
        };

        self.cache.put(symbol.to_string(), result.clone()).await;

        Ok(result)
    }
}

// YahooCurrencyProvider implementation for CurrencyRateProvider
pub struct YahooCurrencyProvider {
    base_url: String,
    cache: Arc<Cache<String, f64>>,
}

impl YahooCurrencyProvider {
    pub fn new(base_url: &str, cache: Arc<Cache<String, f64>>) -> Self {
        YahooCurrencyProvider {
            base_url: base_url.to_string(),
            cache,
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
        if let Some(cached) = self.cache.get(&symbol).await {
            return Ok(cached);
        }

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

        let rate = item.meta.regular_market_price;
        self.cache.put(symbol, rate).await;
        Ok(rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // Tests for YahooFinanceProvider (PriceProvider)
    pub async fn create_mock_server(symbol: &str, mock_response: &str) -> wiremock::MockServer {
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
        let cache = Arc::new(Cache::new());

        let provider = YahooFinanceProvider::new(&mock_server.uri(), cache);
        let result = provider.fetch_price("AAPL").await.unwrap();
        assert_eq!(result.price, 150.65);
        assert_eq!(result.currency, "USD");
        assert!(result.historical.is_empty());
    }

    #[tokio::test]
    async fn test_successful_price_fetch_with_historical_data() {
        let now = chrono::Utc::now();
        let current_price = 150.65;
        let ts_5y = (now - chrono::Duration::days(365 * 5 - 10)).timestamp();
        let p_5y = 100.0;
        let ts_1y = (now - chrono::Duration::days(365 - 10)).timestamp();
        let p_1y = 120.0;
        let ts_1m = (now - chrono::Duration::weeks(4) + chrono::Duration::days(2)).timestamp();
        let p_1m = 130.0;
        let ts_5d = (now - chrono::Duration::days(5) + chrono::Duration::days(1)).timestamp();
        let p_5d = 145.0;

        let mock_response = format!(
            r#"{{
                "chart": {{
                    "result": [{{
                        "meta": {{
                            "regularMarketPrice": {current_price},
                            "currency": "USD"
                        }},
                        "timestamp": [{ts_5y}, {ts_1y}, {ts_1m}, {ts_5d}],
                        "indicators": {{
                            "quote": [{{
                                "close": [{p_5y}, {p_1y}, {p_1m}, {p_5d}]
                            }}]
                        }}
                    }}]
                }}
            }}"#,
        );

        let mock_server = create_mock_server("AAPL", &mock_response).await;
        let cache = Arc::new(Cache::new());

        let provider = YahooFinanceProvider::new(&mock_server.uri(), cache);
        let result = provider.fetch_price("AAPL").await.unwrap();

        assert_eq!(result.price, current_price);
        assert_eq!(result.currency, "USD");
        assert_eq!(result.historical.len(), 6); // 10Y, 5Y, 3Y, 1Y, 1M, 5D

        let expected_change_5y = ((current_price - p_5y) / p_5y) * 100.0;
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

        let expected_change_1y = ((current_price - p_1y) / p_1y) * 100.0;
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

        let expected_change_1m = ((current_price - p_1m) / p_1m) * 100.0;
        assert!(
            (result.historical.get(&HistoricalPeriod::OneMonth).unwrap() - expected_change_1m)
                .abs()
                < 0.001
        );

        let expected_change_5d = ((current_price - p_5d) / p_5d) * 100.0;
        assert!(
            (result.historical.get(&HistoricalPeriod::FiveDays).unwrap() - expected_change_5d)
                .abs()
                < 0.001
        );
    }

    #[tokio::test]
    async fn test_no_price_result_data() {
        let mock_response = r#"{"chart": {"result": []}}"#;
        let mock_server = create_mock_server("INVALID", mock_response).await;
        let cache = Arc::new(Cache::new());

        let provider = YahooFinanceProvider::new(&mock_server.uri(), cache);
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
        let cache = Arc::new(Cache::new());
        let provider = YahooCurrencyProvider::new(&mock_server.uri(), cache);

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
        let cache = Arc::new(Cache::new());
        let provider = YahooCurrencyProvider::new(&mock_server.uri(), cache);

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
        let cache = Arc::new(Cache::new());
        let provider = YahooCurrencyProvider::new(&mock_server.uri(), cache);

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
        let cache = Arc::new(Cache::new());
        let provider = YahooCurrencyProvider::new(&mock_server.uri(), cache);

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
