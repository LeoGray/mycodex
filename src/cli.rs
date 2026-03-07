use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::app::App;
use crate::config::Config;

#[derive(Debug, Parser)]
#[command(
    name = "mycodex",
    version,
    about = "Telegram-driven multi-repo Codex gateway"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Serve {
        #[arg(long)]
        config: PathBuf,
    },
    Check {
        #[arg(long)]
        config: PathBuf,
    },
}

pub async fn run() -> Result<()> {
    init_tracing();

    let cli = Cli::parse();
    match cli.command {
        Command::Serve { config } => {
            let config = Config::load(&config)?;
            let mut app = App::new(config).await?;
            app.run().await
        }
        Command::Check { config } => {
            let config = Config::load(&config)?;
            config.validate()?;
            App::check(config).await
        }
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .try_init();
}
