use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};
use tracing::debug;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Investment {
    #[serde(default)]
    pub symbol: Option<String>,
    #[serde(default)]
    pub isin: Option<String>,
    pub units: f64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Portfolio {
    pub name: String,
    pub investments: Vec<Investment>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct YahooProviderConfig {
    pub base_url: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AmfiProviderConfig {
    pub base_url: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ProvidersConfig {
    pub yahoo: Option<YahooProviderConfig>,
    pub amfi: Option<AmfiProviderConfig>,
}

impl Default for ProvidersConfig {
    fn default() -> Self {
        ProvidersConfig {
            yahoo: Some(YahooProviderConfig {
                base_url: "https://query1.finance.yahoo.com".to_string(),
            }),
            amfi: Some(AmfiProviderConfig {
                base_url: "https://mf.captnemo.in".to_string(),
            }),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AppConfig {
    pub portfolios: Vec<Portfolio>,
    #[serde(default)]
    pub providers: ProvidersConfig,
    pub currency: String,
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        debug!("Loading default config");
        let config_path = Self::default_config_path()?;
        Self::load_from_path(&config_path)
    }

    pub fn default_config_path() -> Result<PathBuf> {
        let proj_dirs = ProjectDirs::from("in", "codito", "xmf")
            .context("Could not determine project directories")?;
        Ok(proj_dirs.config_dir().join("config.yaml"))
    }

    pub fn load_from_path<P: AsRef<std::path::Path>>(path: P) -> Result<Self> {
        let config_str = fs::read_to_string(path.as_ref())
            .with_context(|| format!("Failed to read config file: {}", path.as_ref().display()))?;

        let config: Self = serde_yaml::from_str(&config_str)
            .with_context(|| format!("Failed to parse config file: {}", path.as_ref().display()))?;
        debug!("Successfully loaded config");
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_deserialization() {
        let yaml_str = r#"
portfolios:
  - name: "Tech Stocks"
    investments:
      - symbol: "AAPL"
        units: 10.5
      - symbol: "MSFT"
        units: 5.0
  - name: "Mutual Funds"
    investments:
      - symbol: "MUTF_IN123"
        units: 100.0
currency: "USD"
"#;

        let config: AppConfig = serde_yaml::from_str(yaml_str).expect("Failed to deserialize");
        assert_eq!(config.portfolios.len(), 2);
        assert_eq!(config.portfolios[0].name, "Tech Stocks");
        assert_eq!(config.portfolios[0].investments.len(), 2);
        assert_eq!(
            config.portfolios[0].investments[0].symbol,
            Some("AAPL".to_string())
        );
        assert_eq!(config.portfolios[0].investments[0].units, 10.5);
        assert_eq!(config.portfolios[1].name, "Mutual Funds");
        assert_eq!(
            config.portfolios[0].investments[1].symbol,
            Some("MSFT".to_string())
        );
        assert_eq!(config.portfolios[0].investments[1].units, 5.0);
        assert!(config.providers.yahoo.is_some());
        assert_eq!(
            config.providers.yahoo.unwrap().base_url,
            "https://query1.finance.yahoo.com".to_string()
        );
        // assert_eq!(config.currency, "USD"); // Currency not in test yaml_str

        let yaml_str_with_providers = r#"
portfolios:
  - name: "Test Portfolio"
    investments:
      - symbol: "TEST"
        units: 1.0
providers:
  yahoo:
    base_url: "http://example.com/yahoo"
  amfi:
    base_url: "http://example.com/amfi"
currency: "EUR"
        "#;
        let config_with_providers: AppConfig =
            serde_yaml::from_str(yaml_str_with_providers).unwrap();
        assert!(config_with_providers.providers.yahoo.is_some());
        assert_eq!(
            config_with_providers.providers.yahoo.unwrap().base_url,
            "http://example.com/yahoo"
        );
        assert!(config_with_providers.providers.amfi.is_some());
        assert_eq!(
            config_with_providers.providers.amfi.unwrap().base_url,
            "http://example.com/amfi"
        );
        assert_eq!(config_with_providers.currency, "EUR");
    }
}
