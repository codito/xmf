use crate::core::config::AppConfig;
use anyhow::{Context, Result};
use std::path::Path;

/// Creates a default configuration file with example content at the default location
pub fn setup() -> Result<()> {
    let path = AppConfig::default_config_path()?;

    if path.exists() {
        anyhow::bail!("Configuration file already exists at {}", path.display());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    // Include the example config as a string literal in the binary
    let default_config = include_str!("../../docs/example_config.yaml");

    std::fs::write(&path, default_config)
        .with_context(|| format!("Failed to write config file to {}", path.display()))?;

    tracing::info!("Created default configuration at {}", path.display());
    Ok(())
}

/// Creates a default configuration file with example content at the specified path
pub fn setup_at_path<P: AsRef<Path>>(path: P) -> Result<()> {
    let path = path.as_ref();

    if path.exists() {
        anyhow::bail!("Configuration file already exists at {}", path.display());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    // Include the example config as a string literal in the binary
    let default_config = include_str!("../../docs/example_config.yaml");

    std::fs::write(path, default_config)
        .with_context(|| format!("Failed to write config file to {}", path.display()))?;

    tracing::info!("Created default configuration at {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_setup_creates_config_file() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let config_path = temp_dir.path().join("config.yaml");

        // Test the setup_at_path function
        setup_at_path(&config_path)?;

        // Verify the file was created
        assert!(config_path.exists());

        // Verify the file contains expected content
        let content = fs::read_to_string(&config_path)?;
        assert!(content.contains("portfolios:"));
        assert!(content.contains("providers:"));
        assert!(content.contains("currency:"));
        assert!(content.contains("# Example configuration file for xmf"));

        Ok(())
    }

    #[test]
    fn test_setup_fails_if_config_exists() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let config_path = temp_dir.path().join("config.yaml");

        // Create a file at the config path
        std::fs::write(&config_path, "test")?;

        // Try to run setup - it should fail
        let result = setup_at_path(&config_path);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));

        Ok(())
    }

    #[test]
    fn test_example_config_is_valid_yaml() -> Result<()> {
        // Test that the embedded example config is valid YAML
        let example_config = include_str!("../../docs/example_config.yaml");
        let config: AppConfig = serde_yaml::from_str(example_config)
            .context("Failed to parse example config as YAML")?;

        // Basic validation
        assert!(!config.portfolios.is_empty());
        assert!(config.providers.yahoo.is_some() || config.providers.amfi.is_some());
        assert!(!config.currency.is_empty());

        Ok(())
    }
}
