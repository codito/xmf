use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use std::path::PathBuf;
use xmf::core::log::init_logging;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Enable verbose logging
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Force refresh of cached data
    #[arg(long, global = true)]
    force_refresh: bool,

    /// Path to custom configuration file (overrides default config search)
    #[arg(
        short,
        long,
        global = true,
        value_name = "FILE",
        conflicts_with = "config_name"
    )]
    config_path: Option<PathBuf>,

    /// Use a named configuration from the default config directory
    #[arg(
        short = 'n',
        long = "config-name",
        global = true,
        value_name = "NAME",
        conflicts_with = "config_path"
    )]
    config_name: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

impl From<Commands> for xmf::AppCommand {
    fn from(cmd: Commands) -> xmf::AppCommand {
        match cmd {
            Commands::Summary => xmf::AppCommand::Summary,
            Commands::Change => xmf::AppCommand::Change,
            Commands::Returns => xmf::AppCommand::Returns,
            Commands::Fees => xmf::AppCommand::Fees,
            Commands::Setup => unreachable!("Setup command should be handled separately"),
        }
    }
}

#[derive(Subcommand)]
enum Commands {
    /// Create default configuration
    Setup,
    /// Display portfolio summary
    Summary,
    /// Display price change summary
    Change,
    /// Display CAGR return calculations
    Returns,
    /// Display expense ratios and fees
    Fees,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    init_logging(cli.verbose);

    let config_arg =
        if let Some(name) = &cli.config_name {
            let mut base_path = xmf::core::config::AppConfig::default_config_path()?;
            // Pop the default config file name to get the config directory
            base_path.pop();

            // Check for both yaml and yml extensions
            let extensions = ["yaml", "yml"];
            let path = extensions.iter()
            .map(|ext| {
                let mut path = base_path.clone();
                path.push(format!("{name}.{ext}"));
                path
            })
            .find(|path| path.exists())
            .ok_or_else(|| anyhow::anyhow!(
                "No config file found for name '{}' with extensions {:?} in config directory", 
                name, extensions
            ))?;

            Some(path)
        } else {
            cli.config_path
        };

    let result = match cli.command {
        Some(Commands::Setup) => setup(),
        Some(cmd) => xmf::run_command(cmd.into(), config_arg.as_deref(), cli.force_refresh).await,
        None => {
            Cli::command().print_help()?;
            Ok(())
        }
    };

    if let Err(e) = &result {
        tracing::error!(error = %e, "Application failed");
    }
    result
}

fn setup() -> anyhow::Result<()> {
    use anyhow::Context;

    let path = xmf::core::config::AppConfig::default_config_path()?;

    if path.exists() {
        anyhow::bail!("Configuration file already exists at {}", path.display());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    let default_config = r#"---
portfolios:
  - name: "Example"
    investments: []

providers:
  yahoo:
    base_url: "https://query1.finance.yahoo.com"

currency: "USD"
"#;

    std::fs::write(&path, default_config)
        .with_context(|| format!("Failed to write config file to {}", path.display()))?;

    tracing::info!("Created default configuration at {}", path.display());
    Ok(())
}
