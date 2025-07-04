use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use xmf::log::init_logging;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Enable verbose logging
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Path to optional configuration file
    #[arg(short, long, global = true)]
    config_path: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Create default configuration
    Setup,
    /// Display portfolio summary
    Summary,
    /// Display price change summary
    Change,
    /// Display XIRR return calculations
    Returns,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    init_logging(cli.verbose);

    let result = match cli.command {
        Some(Commands::Setup) => setup(),
        Some(Commands::Summary) => xmf::run(cli.config_path.as_deref()).await,
        Some(Commands::Change) => xmf::change::run(cli.config_path.as_deref()).await,
        Some(Commands::Returns) => xmf::returns::run(cli.config_path.as_deref()).await,
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

    let path = xmf::config::AppConfig::default_config_path()?;

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
