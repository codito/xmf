use std::fs;
use tracing::{error, info};

// Adds automatic logging to test
mod test_utils {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    pub async fn create_mock_server(symbol: &str, mock_response: &str) -> wiremock::MockServer {
        let mock_server = wiremock::MockServer::start().await;
        let url_path = format!("/v8/finance/chart/{symbol}");

        wiremock::Mock::given(method("GET"))
            .and(path(&url_path))
            .respond_with(ResponseTemplate::new(200).set_body_string(mock_response))
            .mount(&mock_server)
            .await;

        mock_server
    }

    // New helper for AMFI mock server in integration tests
    pub async fn create_amfi_mock_server(isin: &str, mock_response: &str) -> wiremock::MockServer {
        let mock_server = MockServer::start().await;
        let url_path = format!("/{isin}"); // AMFI provider uses format!("{}/{}", self.base_url, identifier);

        Mock::given(method("GET"))
            .and(path(&url_path))
            .respond_with(ResponseTemplate::new(200).set_body_string(mock_response))
            .mount(&mock_server)
            .await;

        mock_server
    }
}

#[test_log::test(tokio::test)]
async fn test_real_yahoo_currency_api() {
    use xmf::core::currency::CurrencyRateProvider;
    use xmf::providers::yahoo_finance::YahooCurrencyProvider;

    let base_url = "https://query1.finance.yahoo.com";
    let cache = std::sync::Arc::new(xmf::cache::Cache::new());
    let provider = YahooCurrencyProvider::new(base_url, cache);

    let from_currency = "USD";
    let to_currency = "EUR";
    info!(
        ?from_currency,
        ?to_currency,
        "Fetching currency rate from Yahoo Finance"
    );

    let result = provider.get_rate(from_currency, to_currency).await;

    match result {
        Ok(rate) => {
            info!(?rate, "Received successful currency rate response");
            assert!(rate > 0.0, "Currency rate should be positive");

            info!(
                "Real API Response - {} to {}: {}",
                from_currency, to_currency, rate
            );
        }
        Err(e) => {
            error!("Currency rate API request failed: {e}\n{e:?}");
            panic!("Currency rate API request failed: {e}");
        }
    }
}

// New integration test for AMFI provider
#[test_log::test(tokio::test)]
async fn test_full_app_flow_with_amfi_mock() {
    let isin = "INF789F01XA0";
    let mock_response = r#"{"nav": 125.75}"#;

    // Setup mock server for AMFI
    let mock_server = test_utils::create_amfi_mock_server(isin, mock_response).await;

    // Setup config file with AMFI investment and provider
    let config_file = tempfile::NamedTempFile::new().expect("Failed to create temp file");
    let config_path = config_file.path();
    let config_content = format!(
        r#"
        portfolios:
          - name: "Indian Mutual Funds"
            investments:
              - isin: "{}"
                units: 100.0
        providers:
          amfi:
            base_url: {}
        currency: "INR"
    "#,
        isin,
        mock_server.uri()
    );

    fs::write(config_path, &config_content).expect("Failed to write config file");

    // Run app and verify success
    let result = xmf::run_command(
        xmf::AppCommand::Summary,
        Some(config_path.to_str().unwrap()),
    )
    .await;
    assert!(
        result.is_ok(),
        "Main function failed with: {:?}",
        result.err()
    );
}

#[test_log::test(tokio::test)]
async fn test_full_app_flow_with_mock() {
    use chrono::{Duration, Utc};

    let now = Utc::now();
    let ts_1y = (now - Duration::days(364)).timestamp();
    let price_1y = 150.0;
    let ts_1m = (now - Duration::days(30)).timestamp();
    let price_1m = 165.0;

    // Setup mock server
    let mock_response = format!(
        r#"
    {{
        "chart": {{
            "result": [
                {{
                    "meta": {{
                        "regularMarketPrice": 175.5,
                        "currency": "USD"
                    }},
                    "timestamp": [{ts_1y}, {ts_1m}],
                    "indicators": {{
                        "quote": [{{
                            "close": [{price_1y}, {price_1m}]
                        }}]
                    }}
                }}
            ]
        }}
    }}"#,
    );

    let mock_server = test_utils::create_mock_server("AAPL", &mock_response).await;

    // Setup config file
    let config_file = tempfile::NamedTempFile::new().expect("Failed to create temp file");
    let config_path = config_file.path();
    let config_content = format!(
        r#"
        portfolios:
          - name: "Tech Stocks"
            investments:
              - symbol: "AAPL"
                units: 10.5
        providers:
          yahoo:
            base_url: {}
        currency: "USD"
    "#,
        mock_server.uri()
    );

    fs::write(config_path, &config_content).expect("Failed to write config file");

    // Run app and verify success
    let result = xmf::run_command(
        xmf::AppCommand::Summary,
        Some(config_path.to_str().unwrap()),
    )
    .await;
    assert!(
        result.is_ok(),
        "Main function failed with: {:?}",
        result.err()
    );
}

#[test_log::test(tokio::test)]
async fn test_real_yahoo_finance_api() {
    use xmf::core::price::PriceProvider;
    use xmf::providers::yahoo_finance::YahooFinanceProvider;

    let base_url = "https://query1.finance.yahoo.com";
    let cache = std::sync::Arc::new(xmf::cache::Cache::new());
    let provider = YahooFinanceProvider::new(base_url, cache);

    let symbol = "AAPL";
    info!(?symbol, "Fetching price from Yahoo Finance");

    let result = provider.fetch_price(symbol).await;

    // Log full response and fail test if needed
    match result {
        Ok(price_result) => {
            info!(?price_result, "Received successful price response");
            assert!(price_result.price > 0.0, "Price should be positive");
            assert!(
                !price_result.currency.is_empty(),
                "Currency should not be empty"
            );
            assert!(
                !price_result.historical_prices.is_empty(),
                "Historical data should not be empty"
            );

            info!(
                "Real API Response - {}: {} {}",
                symbol, price_result.price, price_result.currency
            );
        }
        Err(e) => {
            error!("API request failed: {e}\n{e:?}");
            panic!("API request failed: {e}");
        }
    }
}

#[test_log::test(tokio::test)]
async fn test_real_amfi_api() {
    use xmf::core::price::PriceProvider;
    use xmf::providers::amfi_provider::AmfiProvider;

    let base_url = "https://mf.captnemo.in";
    let cache = std::sync::Arc::new(xmf::cache::Cache::new());
    let provider = AmfiProvider::new(base_url, cache);

    // Use same ISIN as unit tests
    let isin = "INF789F01XA0";
    info!(?isin, "Fetching price from AMFI API");

    let result = provider.fetch_price(isin).await;

    match result {
        Ok(price_result) => {
            info!(?price_result, "Received successful price response");
            assert!(price_result.price > 0.0, "Price should be positive");
            assert_eq!(
                price_result.currency, "INR",
                "Currency should be Indian Rupee"
            );
            assert!(
                !price_result.historical_prices.is_empty(),
                "Price history should be non-empty"
            );

            info!(
                "Real AMFI Response - {}: {} {}",
                isin, price_result.price, price_result.currency
            );
        }
        Err(e) => {
            error!("AMFI API request failed: {e}\n{e:?}");
            panic!("AMFI API request failed: {e}");
        }
    }
}
