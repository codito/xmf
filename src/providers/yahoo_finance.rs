use crate::providers::util::with_retry;
use crate::{core::cache::Store, store::KeyValueStore};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, instrument};

use crate::core::cache::KeyValueCollection;
use crate::core::{CurrencyRateProvider, HistoricalPeriod, PriceProvider, PriceResult};
use std::time::Duration;

fn find_closest_price(target_ts: i64, timestamps: &[i64], prices: &[Option<f64>]) -> Option<f64> {
    timestamps
        .iter()
        .position(|ts| *ts >= target_ts)
        .and_then(|index| prices.get(index).and_then(|p| *p))
}

fn extract_historical_prices(chart_item: &PriceChartItem) -> HashMap<HistoricalPeriod, f64> {
    let mut historical_prices = HashMap::new();

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
            None => return historical_prices,
        };

        // For 1-day period: use the second last element (previous day's close)
        // Last element is today's current price, second last is previous close
        if closes.len() >= 2
            && let Some(prev_close) = closes.get(closes.len() - 2).copied().flatten()
        {
            historical_prices.insert(HistoricalPeriod::OneDay, prev_close);
        }

        // Calculate other periods using historical data
        for period in [
            HistoricalPeriod::FiveDays,
            HistoricalPeriod::OneMonth,
            HistoricalPeriod::OneYear,
            HistoricalPeriod::ThreeYears,
            HistoricalPeriod::FiveYears,
            HistoricalPeriod::TenYears,
        ] {
            // Logic is not perfect since we're not excluding weekends and other holidays.
            // Use approximation to avoid multiple API calls to the providers.
            let target_date = reference_date - period.to_duration();
            if let Some(price) = find_closest_price(target_date.timestamp(), timestamps, closes)
                && price > 0.0
            {
                historical_prices.insert(period, price);
            }
        }
    }

    historical_prices
}

// YahooFinanceProvider implementation for PriceProvider
pub struct YahooFinanceProvider {
    base_url: String,
    cache: Arc<dyn KeyValueCollection>,
}

impl YahooFinanceProvider {
    pub fn new(base_url: &str, cache: Arc<KeyValueStore>) -> Self {
        let collection = cache
            .get_collection("yahoo", true /* persist */, true /* create */)
            .unwrap();
        YahooFinanceProvider {
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
    #[serde(alias = "longName")] // full name of the ticker
    short_name: Option<String>,
}

#[async_trait]
impl PriceProvider for YahooFinanceProvider {
    #[instrument(
        name = "YahooPriceFetch",
        skip(self),
        fields(symbol = %symbol)
    )]
    async fn fetch_price(&self, symbol: &str) -> Result<PriceResult> {
        if let Some(cached) = self.cache.get(symbol.as_bytes()).await {
            return Ok(serde_json::from_slice(&cached)?);
        }

        let url = format!(
            "{}/v8/finance/chart/{}?interval=1d&range=10y",
            self.base_url, symbol
        );
        debug!("Requesting price data from {}", url);

        let client = reqwest::Client::builder().user_agent("xmf/1.0").build()?;
        let response = with_retry(|| async { client.get(&url).send().await }, 3, 500)
            .await
            .map_err(|e| anyhow!("Request error: {} for symbol: {} URL: {}", e, symbol, url))?;

        debug!(response = ?response, "Received Yahoo response");

        let data = response.json::<YahooPriceResponse>().await?;
        let item = data
            .chart
            .result
            .first()
            .ok_or_else(|| anyhow!("No price data found for symbol: {}", symbol))?;

        let mut current_price = item.meta.regular_market_price;
        let mut currency = item.meta.currency.clone();
        let short_name = item.meta.short_name.clone();
        let mut daily_prices = Vec::new();
        let mut historical_prices = extract_historical_prices(item);

        if let (Some(timestamps), Some(closes)) = (
            item.timestamp.as_ref(),
            item.indicators
                .as_ref()
                .and_then(|inds| inds.quote.first())
                .and_then(|q| q.close.as_ref()),
        ) {
            for (index, ts) in timestamps.iter().enumerate() {
                if let Some(Some(close)) = closes.get(index) {
                    let date = Utc
                        .timestamp_opt(*ts, 0)
                        .single()
                        .map(|datetime| datetime.date_naive());
                    if let Some(date) = date {
                        daily_prices.push((date, *close));
                    }
                }
            }
        }

        if currency == "GBp" {
            currency = "GBP".to_string();
            current_price /= 100.0;
            for (_, price) in historical_prices.iter_mut() {
                *price /= 100.0;
            }
            for (_, price) in daily_prices.iter_mut() {
                *price /= 100.0;
            }
        }

        let result = PriceResult {
            price: current_price,
            currency,
            historical_prices,
            daily_prices,
            short_name,
        };

        // Cache with short-lived TTL (5 minutes) for stocks
        self.cache
            .put(
                symbol.as_bytes(),
                &serde_json::to_vec(&result).unwrap(),
                Some(Duration::from_secs(300)),
            )
            .await;

        Ok(result)
    }
}

// YahooCurrencyProvider implementation for CurrencyRateProvider
pub struct YahooCurrencyProvider {
    base_url: String,
    cache: Arc<dyn KeyValueCollection>,
}

impl YahooCurrencyProvider {
    pub fn new(base_url: &str, cache: Arc<KeyValueStore>) -> Self {
        let collection = cache
            .get_collection("currency", true /* persist */, true /* create */)
            .unwrap();
        YahooCurrencyProvider {
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
        if let Some(cached) = self.cache.get(symbol.as_bytes()).await {
            return Ok(serde_json::from_slice(&cached)?);
        }

        let endpoint = format!("/v8/finance/chart/{symbol}");
        let url = format!("{}{}", self.base_url, endpoint);
        debug!("Requesting currency rate from {}", url);

        let client = reqwest::Client::builder().user_agent("xmf/1.0").build()?;

        let response = with_retry(|| async { client.get(&url).send().await }, 3, 500)
            .await
            .map_err(|e| anyhow!("Request error: {} for currency pair: {}", e, symbol))?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "HTTP error: {} for currency pair: {}",
                response.status(),
                symbol
            ));
        }

        let text = response
            .text()
            .await
            .map_err(|e| anyhow!("Failed to read response text: {}", e))?;

        let data: YahooCurrencyResponse = serde_json::from_str(&text)
            .map_err(|e| anyhow!("Failed to parse JSON response for {}: {}", symbol, e))?;

        let item = data
            .chart
            .result
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("No rate data found for currency pair: {}", symbol))?;

        let rate = item.meta.regular_market_price;
        self.cache
            .put(
                symbol.as_bytes(),
                &serde_json::to_vec(&rate).unwrap(),
                Some(Duration::from_secs(300)),
            )
            .await;
        Ok(rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::memory::MemoryCollection;
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
                        "currency": "USD",
                        "shortName": "Apple Inc."
                    }
                }]
            }
        }"#;

        let mock_server = create_mock_server("AAPL", mock_response).await;
        let cache = Arc::new(MemoryCollection::new());

        let provider = YahooFinanceProvider::new_with_collection(&mock_server.uri(), cache);
        let result = provider.fetch_price("AAPL").await.unwrap();
        assert_eq!(result.price, 150.65);
        assert_eq!(result.currency, "USD");
        assert_eq!(result.short_name, Some("Apple Inc.".to_string()));
        assert!(result.historical_prices.is_empty());
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
        let ts_prev = (now - chrono::Duration::days(1)).timestamp();
        let p_prev = 140.0;
        let ts_curr = now.timestamp();

        let mock_response = format!(
            r#"{{
                "chart": {{
                    "result": [{{
                        "meta": {{
                            "regularMarketPrice": {current_price},
                            "currency": "USD",
                            "shortName": "Apple Inc."
                        }},
                        "timestamp": [{ts_5y}, {ts_1y}, {ts_1m}, {ts_5d}, {ts_prev}, {ts_curr}],
                        "indicators": {{
                            "quote": [{{
                                "close": [{p_5y}, {p_1y}, {p_1m}, {p_5d}, {p_prev}, {current_price}]
                            }}]
                        }}
                    }}]
                }}
            }}"#,
        );

        let mock_server = create_mock_server("AAPL", &mock_response).await;
        let cache = Arc::new(MemoryCollection::new());

        let provider = YahooFinanceProvider::new_with_collection(&mock_server.uri(), cache);
        let result = provider.fetch_price("AAPL").await.unwrap();

        assert_eq!(result.price, current_price);
        assert_eq!(result.currency, "USD");
        assert_eq!(result.short_name, Some("Apple Inc.".to_string()));

        // We should have 1D, 5D, 1M, 1Y, 3Y, 5Y, 10Y: 7 periods
        assert_eq!(result.historical_prices.len(), 7);

        assert_eq!(result.historical_prices[&HistoricalPeriod::OneDay], p_prev);

        assert!(
            (result
                .historical_prices
                .get(&HistoricalPeriod::FiveYears)
                .unwrap()
                - p_5y)
                .abs()
                < 0.001
        );
        assert!(
            (result
                .historical_prices
                .get(&HistoricalPeriod::TenYears)
                .unwrap()
                - p_5y)
                .abs()
                < 0.001
        );

        assert!(
            (result
                .historical_prices
                .get(&HistoricalPeriod::OneYear)
                .unwrap()
                - p_1y)
                .abs()
                < 0.001
        );
        assert!(
            (result
                .historical_prices
                .get(&HistoricalPeriod::ThreeYears)
                .unwrap()
                - p_1y)
                .abs()
                < 0.001
        );

        assert!(
            (result
                .historical_prices
                .get(&HistoricalPeriod::OneMonth)
                .unwrap()
                - p_1m)
                .abs()
                < 0.001
        );

        assert!(
            (result
                .historical_prices
                .get(&HistoricalPeriod::FiveDays)
                .unwrap()
                - p_5d)
                .abs()
                < 0.001
        );

        assert_eq!(result.daily_prices.len(), 6);
        let expected_dates = [ts_5y, ts_1y, ts_1m, ts_5d, ts_prev, ts_curr];
        for (index, (date, _price)) in result.daily_prices.iter().enumerate() {
            let expected_ts = expected_dates[index];
            let expected_date = Utc
                .timestamp_opt(expected_ts, 0)
                .single()
                .unwrap()
                .date_naive();
            assert_eq!(*date, expected_date);
        }
    }

    #[tokio::test]
    async fn test_no_price_result_data() {
        let mock_response = r#"{"chart": {"result": []}}"#;
        let mock_server = create_mock_server("INVALID", mock_response).await;
        let cache = Arc::new(MemoryCollection::new());

        let provider = YahooFinanceProvider::new_with_collection(&mock_server.uri(), cache);
        let result = provider.fetch_price("INVALID").await;
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "No price data found for symbol: INVALID"
        );
    }

    #[tokio::test]
    async fn test_price_fetch_normalizes_gbp_to_gbp() {
        let now = chrono::Utc::now();
        let current_price = 15065.0; // in pence
        let ts_prev = (now - chrono::Duration::days(1)).timestamp();
        let p_prev = 15000.0; // in pence
        let ts_curr = now.timestamp();
        let ts_1y = (now - chrono::Duration::days(365 - 10)).timestamp();
        let p_1y = 12000.0; // in pence

        let mock_response = format!(
            r#"{{
                "chart": {{
                    "result": [{{
                        "meta": {{
                            "regularMarketPrice": {current_price},
                            "currency": "GBp",
                            "shortName": "UK STOCK PLC"
                        }},
                        "timestamp": [{ts_1y}, {ts_prev}, {ts_curr}],
                        "indicators": {{
                            "quote": [{{
                                "close": [{p_1y}, {p_prev}, {current_price}]
                            }}]
                        }}
                    }}]
                }}
            }}"#,
        );

        let mock_server = create_mock_server("UK.L", &mock_response).await;
        let cache = Arc::new(MemoryCollection::new());

        let provider = YahooFinanceProvider::new_with_collection(&mock_server.uri(), cache);
        let result = provider.fetch_price("UK.L").await.unwrap();

        assert_eq!(result.currency, "GBP");
        assert!((result.price - 150.65).abs() < 0.001);

        let hist_1d = result
            .historical_prices
            .get(&HistoricalPeriod::OneDay)
            .unwrap();
        assert!((hist_1d - 150.00).abs() < 0.001);

        let hist_1y = result
            .historical_prices
            .get(&HistoricalPeriod::OneYear)
            .unwrap();
        assert!((hist_1y - 120.00).abs() < 0.001);

        // Check normalized daily prices
        for (_, price) in &result.daily_prices {
            assert!(price > &1.0); // Prices should be in pounds (GBP)
        }
    }

    // Tests for YahooCurrencyProvider (CurrencyRateProvider)
    #[tokio::test]
    async fn test_successful_rate_fetch() {
        let mock_server = MockServer::start().await;
        let cache = Arc::new(MemoryCollection::new());
        let provider = YahooCurrencyProvider::new_with_collection(&mock_server.uri(), cache);

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
        let cache = Arc::new(MemoryCollection::new());
        let provider = YahooCurrencyProvider::new_with_collection(&mock_server.uri(), cache);

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
        let cache = Arc::new(MemoryCollection::new());
        let provider = YahooCurrencyProvider::new_with_collection(&mock_server.uri(), cache);

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
        let cache = Arc::new(MemoryCollection::new());
        let provider = YahooCurrencyProvider::new_with_collection(&mock_server.uri(), cache);

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
