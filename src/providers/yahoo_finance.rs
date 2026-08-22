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

fn find_prev_close(timestamps: &[i64], closes: &[Option<f64>]) -> Option<f64> {
    if timestamps.is_empty() || closes.is_empty() {
        return None;
    }

    let reference_date = Utc
        .timestamp_opt(*timestamps.last()?, 0)
        .single()?
        .date_naive();
    for i in (0..timestamps.len() - 1).rev() {
        if let Some(ts) = Utc.timestamp_opt(*timestamps.get(i)?, 0).single()
            && ts.date_naive() < reference_date
            && let Some(close) = closes.get(i).and_then(|c| *c)
        {
            return Some(close);
        }
    }
    None
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

        if let Some(prev_close) = find_prev_close(timestamps, closes) {
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

    /// Resolves a search query (e.g. an ISIN) to a Yahoo fund symbol.
    ///
    /// Prefers the first mutual fund quote; falls back to the first quote
    /// marked as available on Yahoo Finance. The mapping is cached
    /// persistently since symbols don't change.
    #[instrument(name = "YahooSymbolSearch", skip(self), fields(query = %query))]
    async fn search_fund_symbol(&self, query: &str) -> Result<String> {
        let cache_key = format!("isin:{query}");
        if let Some(cached) = self.cache.get(cache_key.as_bytes()).await {
            return Ok(String::from_utf8_lossy(&cached).into_owned());
        }

        let url = format!("{}/v1/finance/search?q={}", self.base_url, query);
        debug!("Searching fund symbol from {}", url);

        let client = reqwest::Client::builder().user_agent("xmf/1.0").build()?;
        let response = with_retry(|| async { client.get(&url).send().await }, 3, 500)
            .await
            .map_err(|e| anyhow!("Request error: {} for search query: {}", e, query))?;

        let data: YahooSearchResponse = response
            .json()
            .await
            .map_err(|e| anyhow!("Failed to parse search response for {}: {}", query, e))?;

        let symbol = data
            .quotes
            .iter()
            .find(|q| q.quote_type.as_deref() == Some("MUTUALFUND"))
            .or_else(|| data.quotes.iter().find(|q| q.is_yahoo_finance))
            .and_then(|q| q.symbol.clone())
            .ok_or_else(|| anyhow!("No fund symbol found for search query: {}", query))?;

        // Cache the resolved mapping persistently (no TTL).
        self.cache
            .put(cache_key.as_bytes(), symbol.as_bytes(), None)
            .await;

        Ok(symbol)
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
    #[serde(default, alias = "regularMarketTime")]
    regular_market_time: Option<i64>,
    currency: String,
    #[serde(alias = "longName")] // full name of the ticker
    short_name: Option<String>,
}

#[derive(Deserialize, Debug)]
struct YahooSearchResponse {
    quotes: Vec<SearchQuote>,
}

#[derive(Deserialize, Debug)]
struct SearchQuote {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(alias = "quoteType")]
    quote_type: Option<String>,
    #[serde(default, alias = "isYahooFinance")]
    is_yahoo_finance: bool,
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

        // Reconcile the meta quote with the price series so that the
        // reported price and its date always agree:
        // - Meta newer than any valid close (e.g. a null latest candle):
        //   extend the series, mirroring how the AMFI provider appends the
        //   latest NAV.
        // - Meta older than the newest close (mutual fund quotes can lag a
        //   NAV day behind): prefer the series close.
        // - Same day or missing timestamp: keep the meta price as-is.
        if let Some((series_date, series_close)) = daily_prices.last() {
            let meta_date = item
                .meta
                .regular_market_time
                .and_then(|ts| Utc.timestamp_opt(ts, 0).single())
                .map(|dt| dt.date_naive());
            match meta_date {
                Some(d) if d > *series_date => daily_prices.push((d, current_price)),
                Some(d) if d < *series_date => current_price = *series_close,
                _ => {}
            }
        }

        if currency == "GBp" {
            currency = "GBP".to_string();
            current_price /= 100.0;
            for price in historical_prices.values_mut() {
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

// YahooIsinPriceProvider: resolves an ISIN to a Yahoo fund symbol via the
// search endpoint and delegates price fetching to YahooFinanceProvider.
pub struct YahooIsinPriceProvider {
    yahoo: Arc<YahooFinanceProvider>,
}

impl YahooIsinPriceProvider {
    pub fn new(yahoo: Arc<YahooFinanceProvider>) -> Self {
        Self { yahoo }
    }
}

#[async_trait]
impl PriceProvider for YahooIsinPriceProvider {
    #[instrument(
        name = "YahooIsinPriceFetch",
        skip(self),
        fields(symbol = %identifier)
    )]
    async fn fetch_price(&self, identifier: &str) -> Result<PriceResult> {
        let symbol = self.yahoo.search_fund_symbol(identifier).await?;
        self.yahoo.fetch_price(&symbol).await
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
                        "longName": "Apple Inc.",
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
                            "longName": "Apple Inc.",
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
    async fn test_find_prev_close_market_closed() {
        let now = chrono::Utc::now();

        let ts_5 = (now - chrono::Duration::days(5)).timestamp();
        let ts_4 = (now - chrono::Duration::days(4)).timestamp();
        let ts_3 = (now - chrono::Duration::days(3)).timestamp();
        let ts_2 = (now - chrono::Duration::days(2)).timestamp();
        let ts_1 = (now - chrono::Duration::days(1)).timestamp();
        let ts_0 = now.timestamp();

        let p_5 = 100.0;
        let p_4 = 102.0;
        let p_3 = 101.0;
        let p_2 = 103.0;
        let p_1 = 105.0;
        let current_price = 105.0;

        let mock_response = format!(
            r#"{{
                "chart": {{
                    "result": [{{
                        "meta": {{
                            "regularMarketPrice": {current_price},
                            "currency": "USD",
                            "longName": "Test Corp",
                            "shortName": "Test Corp"
                        }},
                        "timestamp": [{ts_5}, {ts_4}, {ts_3}, {ts_2}, {ts_1}, {ts_0}],
                        "indicators": {{
                            "quote": [{{
                                "close": [{p_5}, {p_4}, {p_3}, {p_2}, {p_1}, {current_price}]
                            }}]
                        }}
                    }}]
                }}
            }}"#,
        );

        let mock_server = create_mock_server("TEST", &mock_response).await;
        let cache = Arc::new(MemoryCollection::new());

        let provider = YahooFinanceProvider::new_with_collection(&mock_server.uri(), cache);
        let result = provider.fetch_price("TEST").await.unwrap();

        assert_eq!(result.historical_prices[&HistoricalPeriod::OneDay], p_1);
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
                            "longName": "UK STOCK PLC III",
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

    #[tokio::test]
    async fn test_stale_meta_quote_uses_newest_series_close() {
        // Mutual fund style: the meta quote is one NAV day older than the
        // last close in the series.
        let now = chrono::Utc::now();
        let ts_prev = (now - chrono::Duration::days(1)).timestamp();
        let ts_curr = now.timestamp();

        let mock_response = format!(
            r#"{{
                "chart": {{
                    "result": [{{
                        "meta": {{
                            "regularMarketPrice": 100.0,
                            "regularMarketTime": {ts_prev},
                            "currency": "INR"
                        }},
                        "timestamp": [{ts_prev}, {ts_curr}],
                        "indicators": {{
                            "quote": [{{ "close": [100.0, 101.5] }}]
                        }}
                    }}]
                }}
            }}"#
        );

        let mock_server = create_mock_server("FUND.BO", &mock_response).await;
        let cache = Arc::new(MemoryCollection::new());
        let provider = YahooFinanceProvider::new_with_collection(&mock_server.uri(), cache);
        let result = provider.fetch_price("FUND.BO").await.unwrap();
        assert_eq!(result.price, 101.5);
        // The reported date must match the adopted series close.
        assert_eq!(result.as_of_date(), Some(now.date_naive()));
    }

    #[tokio::test]
    async fn test_same_day_meta_quote_is_kept() {
        // Live equity style: meta quote is dated the same day as the last
        // series close, so it wins.
        let now = chrono::Utc::now();
        let ts_prev = (now - chrono::Duration::days(1)).timestamp();
        let ts_curr = now.timestamp();

        let mock_response = format!(
            r#"{{
                "chart": {{
                    "result": [{{
                        "meta": {{
                            "regularMarketPrice": 105.0,
                            "regularMarketTime": {ts_curr},
                            "currency": "USD"
                        }},
                        "timestamp": [{ts_prev}, {ts_curr}],
                        "indicators": {{
                            "quote": [{{ "close": [100.0, 104.0] }}]
                        }}
                    }}]
                }}
            }}"#
        );

        let mock_server = create_mock_server("MSFT", &mock_response).await;
        let cache = Arc::new(MemoryCollection::new());
        let provider = YahooFinanceProvider::new_with_collection(&mock_server.uri(), cache);
        let result = provider.fetch_price("MSFT").await.unwrap();
        assert_eq!(result.price, 105.0);
    }

    #[tokio::test]
    async fn test_meta_newer_than_series_extends_timeseries() {
        // LSE style (e.g. IWDA.L): the newest candle has a null close, so
        // the fresh meta quote must extend the series to keep the reported
        // price and its date consistent.
        let now = chrono::Utc::now();
        let ts_prev = (now - chrono::Duration::days(1)).timestamp();
        let ts_curr = now.timestamp();

        let mock_response = format!(
            r#"{{
                "chart": {{
                    "result": [{{
                        "meta": {{
                            "regularMarketPrice": 147.71,
                            "regularMarketTime": {ts_curr},
                            "currency": "USD"
                        }},
                        "timestamp": [{ts_prev}, {ts_curr}],
                        "indicators": {{
                            "quote": [{{ "close": [147.39, null] }}]
                        }}
                    }}]
                }}
            }}"#
        );

        let mock_server = create_mock_server("IWDA.L", &mock_response).await;
        let cache = Arc::new(MemoryCollection::new());
        let provider = YahooFinanceProvider::new_with_collection(&mock_server.uri(), cache);
        let result = provider.fetch_price("IWDA.L").await.unwrap();
        assert_eq!(result.price, 147.71);
        assert_eq!(result.as_of_date(), Some(now.date_naive()));
        // The null close is skipped; the meta quote fills today's slot.
        assert_eq!(result.daily_prices.len(), 2);
        assert_eq!(
            result.daily_prices.last(),
            Some(&(now.date_naive(), 147.71))
        );
    }

    // Tests for symbol search and YahooIsinPriceProvider
    async fn mount_search_mock(mock_server: &MockServer, body: &str, times: Option<u64>) {
        let mock = Mock::given(method("GET"))
            .and(path("/v1/finance/search"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body));
        if let Some(times) = times {
            mock.up_to_n_times(times).mount(mock_server).await;
        } else {
            mock.mount(mock_server).await;
        }
    }

    #[tokio::test]
    async fn test_search_fund_symbol_prefers_mutual_fund() {
        let mock_server = MockServer::start().await;
        mount_search_mock(
            &mock_server,
            r#"{"quotes":[
                {"symbol":"FOO.BO","quoteType":"EQUITY","isYahooFinance":true},
                {"symbol":"0P0000XW6V.BO","quoteType":"MUTUALFUND","isYahooFinance":true}
            ]}"#,
            None,
        )
        .await;

        let cache = Arc::new(MemoryCollection::new());
        let provider = YahooFinanceProvider::new_with_collection(&mock_server.uri(), cache);
        let symbol = provider.search_fund_symbol("INF179KB1HU9").await.unwrap();
        assert_eq!(symbol, "0P0000XW6V.BO");
    }

    #[tokio::test]
    async fn test_search_fund_symbol_no_results() {
        let mock_server = MockServer::start().await;
        mount_search_mock(&mock_server, r#"{"quotes":[]}"#, None).await;

        let cache = Arc::new(MemoryCollection::new());
        let provider = YahooFinanceProvider::new_with_collection(&mock_server.uri(), cache);
        let result = provider.search_fund_symbol("UNKNOWN000").await;
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "No fund symbol found for search query: UNKNOWN000"
        );
    }

    #[tokio::test]
    async fn test_search_fund_symbol_caches_mapping() {
        let mock_server = MockServer::start().await;
        // Only one request is allowed to succeed; a second network hit would
        // return no results and fail the lookup.
        mount_search_mock(
            &mock_server,
            r#"{"quotes":[{"symbol":"0P0000XW6V.BO","quoteType":"MUTUALFUND","isYahooFinance":true}]}"#,
            Some(1),
        )
        .await;
        mount_search_mock(&mock_server, r#"{"quotes":[]}"#, None).await;

        let cache = Arc::new(MemoryCollection::new());
        let provider = YahooFinanceProvider::new_with_collection(&mock_server.uri(), cache);
        let first = provider.search_fund_symbol("INF179KB1HU9").await.unwrap();
        let second = provider.search_fund_symbol("INF179KB1HU9").await.unwrap();
        assert_eq!(first, "0P0000XW6V.BO");
        assert_eq!(second, first);
    }

    #[tokio::test]
    async fn test_isin_provider_resolves_and_fetches_price() {
        let now = chrono::Utc::now();
        let chart_response = format!(
            r#"{{
                "chart": {{
                    "result": [{{
                        "meta": {{
                            "regularMarketPrice": 6275.03,
                            "currency": "INR",
                            "instrumentType": "MUTUALFUND",
                            "longName": "HDFC Money Market Dir Gr",
                            "shortName": "HDFC Money Market Dir Gr"
                        }},
                        "timestamp": [{ts}],
                        "indicators": {{
                            "quote": [{{ "close": [6275.03] }}]
                        }}
                    }}]
                }}
            }}"#,
            ts = now.timestamp(),
        );

        let mock_server = MockServer::start().await;
        mount_search_mock(
            &mock_server,
            r#"{"quotes":[{"symbol":"0P0000XW6V.BO","quoteType":"MUTUALFUND","isYahooFinance":true}]}"#,
            None,
        )
        .await;
        Mock::given(method("GET"))
            .and(path("/v8/finance/chart/0P0000XW6V.BO"))
            .respond_with(ResponseTemplate::new(200).set_body_string(chart_response))
            .mount(&mock_server)
            .await;

        let cache = Arc::new(MemoryCollection::new());
        let yahoo = Arc::new(YahooFinanceProvider::new_with_collection(
            &mock_server.uri(),
            cache,
        ));
        let provider = YahooIsinPriceProvider::new(yahoo);

        let result = provider.fetch_price("INF179KB1HU9").await.unwrap();
        assert_eq!(result.price, 6275.03);
        assert_eq!(result.currency, "INR");
        assert_eq!(
            result.short_name,
            Some("HDFC Money Market Dir Gr".to_string())
        );
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
