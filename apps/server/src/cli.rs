use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::app::App as ServerApp;
use crate::app_cli::{AppCommand, run_app_command};
use crate::config::Config;
use crate::onboard::OnboardOptions;
use crate::pairing::{PairingCommand, run_pairing};
use crate::platform::{default_config_path, default_env_path, default_service_path};

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
        #[arg(long, default_value_os_t = default_config_path())]
        config: PathBuf,
    },
    Check {
        #[arg(long, default_value_os_t = default_config_path())]
        config: PathBuf,
    },
    Onboard {
        #[arg(long, default_value_os_t = default_config_path())]
        config: PathBuf,
        #[arg(long, default_value_os_t = default_env_path())]
        env_path: PathBuf,
        #[arg(long, default_value_os_t = default_service_path())]
        service_path: PathBuf,
    },
    Pairing {
        #[arg(long, default_value_os_t = default_config_path())]
        config: PathBuf,
        #[command(subcommand)]
        command: PairingCommand,
    },
    App {
        #[arg(long, default_value_os_t = default_config_path())]
        config: PathBuf,
        #[command(subcommand)]
        command: AppCommand,
    },
}

pub async fn run() -> Result<()> {
    init_tracing();

    let cli = Cli::parse();
    match cli.command {
        Command::Serve { config } => {
            let config = Config::load(&config)?;
            let mut app = ServerApp::new(config).await?;
            app.run().await
        }
        Command::Check { config } => {
            let config = Config::load(&config)?;
            config.validate()?;
            ServerApp::check(config).await
        }
        Command::Onboard {
            config,
            env_path,
            service_path,
        } => {
            crate::onboard::run(OnboardOptions {
                config_path: config,
                env_path,
                service_path,
            })
            .await
        }
        Command::Pairing { config, command } => run_pairing(config, command),
        Command::App { config, command } => run_app_command(config, command),
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
