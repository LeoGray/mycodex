use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::app::App;
use crate::config::Config;
use crate::onboard::OnboardOptions;
use crate::pairing::{PairingCommand, run_pairing};

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

fn default_config_path() -> PathBuf {
    PathBuf::from("/etc/mycodex/config.toml")
}

fn default_env_path() -> PathBuf {
    PathBuf::from("/etc/mycodex/mycodex.env")
}

fn default_service_path() -> PathBuf {
    PathBuf::from("/etc/systemd/system/mycodex.service")
}
