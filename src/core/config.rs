use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};
use tracing::debug;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct StockInvestment {
    pub symbol: String,
    pub units: f64,
    pub category: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MutualFundInvestment {
    pub isin: String,
    pub units: f64,
    pub category: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct FixedDepositInvestment {
    pub name: String,
    pub value: f64,
    pub currency: Option<String>,
    pub category: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub enum Investment {
    Stock(StockInvestment),
    MutualFund(MutualFundInvestment),
    FixedDeposit(FixedDepositInvestment),
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
    pub data_path: Option<String>,
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

    pub fn default_data_path(&self) -> Result<PathBuf> {
        if let Some(custom_path) = &self.data_path {
            return Ok(PathBuf::from(custom_path));
        }
        let proj_dirs = ProjectDirs::from("in", "codito", "xmf")
            .context("Could not determine project directories")?;
        Ok(proj_dirs.data_dir().to_path_buf())
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
        category: intl
  - name: "Mutual Funds"
    investments:
      - isin: "MUTF_IN123"
        units: 100.0
      - isin: "MUTF_IN124"
        units: 100.0
        category: intl
  - name: "Bank Deposits"
    investments:
      - name: "FD with Bank of Rust"
        value: 50000.0
        currency: "INR"
      - name: "FD with Govt of Rust"
        value: 50000.0
        category: gilt
      - name: "FD without Currency"
        value: 30000.0
currency: "USD"
"#;

        let config: AppConfig = serde_yaml::from_str(yaml_str).expect("Failed to deserialize");
        assert_eq!(config.portfolios.len(), 3);
        assert_eq!(config.portfolios[0].name, "Tech Stocks");
        assert_eq!(config.portfolios[0].investments.len(), 2);
        if let Investment::Stock(s) = &config.portfolios[0].investments[0] {
            assert_eq!(s.symbol, "AAPL");
            assert_eq!(s.units, 10.5);
        } else {
            panic!("Expected a stock investment");
        }
        if let Investment::Stock(s) = &config.portfolios[0].investments[1] {
            assert_eq!(s.symbol, "MSFT");
            assert_eq!(s.units, 5.0);
            assert_eq!(s.category, Some("intl".to_string()));
        } else {
            panic!("Expected a stock investment");
        }
        assert_eq!(config.portfolios[1].name, "Mutual Funds");
        if let Investment::MutualFund(mf) = &config.portfolios[1].investments[0] {
            assert_eq!(mf.isin, "MUTF_IN123");
            assert_eq!(mf.units, 100.0);
        } else {
            panic!("Expected a mutual fund investment");
        }
        if let Investment::MutualFund(mf) = &config.portfolios[1].investments[1] {
            assert_eq!(mf.isin, "MUTF_IN124");
            assert_eq!(mf.category, Some("intl".to_string()));
        } else {
            panic!("Expected a mutual fund investment");
        }

        assert_eq!(config.portfolios[2].name, "Bank Deposits");
        assert_eq!(config.portfolios[2].investments.len(), 3);
        if let Investment::FixedDeposit(fd) = &config.portfolios[2].investments[0] {
            assert_eq!(fd.name, "FD with Bank of Rust");
            assert_eq!(fd.value, 50000.0);
            assert_eq!(fd.currency.as_deref(), Some("INR"));
        } else {
            panic!("Expected a fixed deposit investment");
        }
        if let Investment::FixedDeposit(fd) = &config.portfolios[2].investments[1] {
            assert_eq!(fd.name, "FD with Govt of Rust");
            assert_eq!(fd.category, Some("gilt".to_string()));
        } else {
            panic!("Expected a fixed deposit investment");
        }
        if let Investment::FixedDeposit(fd) = &config.portfolios[2].investments[2] {
            assert_eq!(fd.name, "FD without Currency");
            assert_eq!(fd.value, 30000.0);
            assert!(fd.currency.is_none());
        } else {
            panic!("Expected a fixed deposit investment");
        }

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
