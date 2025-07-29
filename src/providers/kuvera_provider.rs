use super::util::with_retry;
use crate::core::{
    cache::Cache,
    metadata::{FundMetadata, MetadataProvider},
};
use anyhow::{Context, anyhow};
use async_trait::async_trait;
use chrono::NaiveDate;
use serde::Deserialize;
use std::sync::Arc;
use tracing::error;

#[derive(Debug, Deserialize)]
struct KuveraResponse {
    #[serde(rename = "ISIN")]
    isin: String,
    fund_type: String,
    fund_category: String,
    #[serde(rename = "expense_ratio")]
    expense_ratio: String,
    expense_ratio_date: String,
    aum: f64,
    fund_rating: Option<u8>,          // Changed to Option
    fund_rating_date: Option<String>, // Changed to Option
    category: String,
}

pub struct KuveraProvider {
    base_url: String,
    cache: Arc<Cache<String, FundMetadata>>,
}

impl KuveraProvider {
    pub fn new(base_url: &str, cache: Arc<Cache<String, FundMetadata>>) -> Self {
        Self {
            base_url: base_url.to_string(),
            cache,
        }
    }

    fn parse_api_date(date_str: &str) -> anyhow::Result<NaiveDate> {
        NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
            .with_context(|| format!("Failed to parse date: {date_str}"))
    }
}

#[async_trait]
impl MetadataProvider for KuveraProvider {
    async fn fetch_metadata(&self, identifier: &str) -> anyhow::Result<FundMetadata> {
        if let Some(cached) = self.cache.get(&identifier.to_string()).await {
            return Ok(cached);
        }

        let url = format!("{}/kuvera/{}", self.base_url, identifier);
        let response = with_retry(|| async { reqwest::get(&url).await }, 3, 500)
            .await
            .context("Metadata request failed")?;

        let response_text = response
            .text()
            .await
            .context("Failed to get response text")?;

        let funds: Vec<KuveraResponse> = match serde_json::from_str(&response_text) {
            Ok(data) => data,
            Err(e) => {
                error!(
                    error = ?e,
                    response = %response_text,
                    "Failed to parse metadata response"
                );
                return Err(e).context("Failed to parse metadata response");
            }
        };

        let fund = funds.first().ok_or_else(|| anyhow!("Empty funds array"))?;

        let metadata = FundMetadata {
            isin: fund.isin.clone(),
            fund_type: fund.fund_type.clone(),
            fund_category: fund.fund_category.clone(),
            expense_ratio: fund
                .expense_ratio
                .parse()
                .context("Invalid expense_ratio")?,
            expense_ratio_date: Self::parse_api_date(&fund.expense_ratio_date)?,
            aum: fund.aum,
            fund_rating: fund.fund_rating,
            fund_rating_date: match &fund.fund_rating_date {
                Some(date_str) => Some(Self::parse_api_date(date_str)?),
                None => None,
            },
            category: fund.category.clone(),
        };

        self.cache
            .put(identifier.to_string(), metadata.clone())
            .await;
        Ok(metadata)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    async fn create_mock_server(identifier: &str, mock_response: &str) -> wiremock::MockServer {
        let mock_server = wiremock::MockServer::start().await;
        let request_path = format!("/kuvera/{identifier}");

        Mock::given(method("GET"))
            .and(path(request_path))
            .respond_with(ResponseTemplate::new(200).set_body_string(mock_response))
            .mount(&mock_server)
            .await;

        mock_server
    }

    const TEST_ID: &str = "INF109K01R14";
    const MOCK_JSON: &str = r#"[
        {
            "isin": "INF194K01U07",
            "fund_type": "Debt",
            "fund_category": "Short Duration Fund",
            "expense_ratio": "0.33",
            "expense_ratio_date": "2025-06-30",
            "aum": 107715.0,
            "fund_rating": 4,
            "fund_rating_date": "2025-06-30",
            "category": "Debt - Bonds"
        }
    ]"#;

    const MOCK_JSON_NO_RATING: &str = r#"[
        {
            "isin": "INF194K01U07",
            "fund_type": "Debt",
            "fund_category": "Short Duration Fund",
            "expense_ratio": "0.33",
            "expense_ratio_date": "2025-06-30",
            "aum": 107715.0,
            "fund_rating": null,
            "fund_rating_date": null,
            "category": "Debt - Bonds"
        }
    ]"#;

    #[tokio::test]
    async fn test_fetch_metadata() {
        let mock_server = create_mock_server(TEST_ID, MOCK_JSON).await;
        let cache = Arc::new(Cache::<String, FundMetadata>::new());
        let provider = KuveraProvider::new(&mock_server.uri(), cache);

        let meta = provider.fetch_metadata(TEST_ID).await.unwrap();

        assert_eq!(meta.isin, "INF194K01U07");
        assert_eq!(meta.fund_type, "Debt");
        assert_eq!(meta.fund_category, "Short Duration Fund");
        assert_eq!(meta.expense_ratio, 0.33);
        assert_eq!(meta.expense_ratio_date.year(), 2025);
        assert_eq!(meta.aum, 107715.0);
        assert_eq!(meta.fund_rating, Some(4));
        assert_eq!(meta.fund_rating_date.unwrap().year(), 2025);
        assert_eq!(meta.category, "Debt - Bonds");
    }

    #[tokio::test]
    async fn test_fetch_metadata_without_rating() {
        let mock_server = create_mock_server(TEST_ID, MOCK_JSON_NO_RATING).await;
        let cache = Arc::new(Cache::<String, FundMetadata>::new());
        let provider = KuveraProvider::new(&mock_server.uri(), cache);

        let meta = provider.fetch_metadata(TEST_ID).await.unwrap();

        assert_eq!(meta.isin, "INF194K01U07");
        assert_eq!(meta.fund_type, "Debt");
        assert_eq!(meta.fund_category, "Short Duration Fund");
        assert_eq!(meta.expense_ratio, 0.33);
        assert_eq!(meta.expense_ratio_date.year(), 2025);
        assert_eq!(meta.aum, 107715.0);
        assert!(meta.fund_rating.is_none());
        assert!(meta.fund_rating_date.is_none());
        assert_eq!(meta.category, "Debt - Bonds");
    }

    #[tokio::test]
    async fn test_cache_hit() {
        let mock_server = create_mock_server(TEST_ID, MOCK_JSON).await;
        let cache = Arc::new(Cache::<String, FundMetadata>::new());
        let provider = KuveraProvider::new(&mock_server.uri(), cache);

        // First call should hit network
        provider.fetch_metadata(TEST_ID).await.unwrap();
        // Second call should hit cache
        provider.fetch_metadata(TEST_ID).await.unwrap();
    }
}
