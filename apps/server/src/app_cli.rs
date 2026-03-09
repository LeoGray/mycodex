use std::path::PathBuf;

use anyhow::Result;
use clap::Subcommand;

use crate::app_auth::AppAuthStore;
use crate::config::Config;

#[derive(Debug, Clone, Subcommand)]
pub enum AppCommand {
    Pairing {
        #[command(subcommand)]
        command: AppPairingCommand,
    },
    Devices {
        #[command(subcommand)]
        command: AppDevicesCommand,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum AppPairingCommand {
    List,
    Approve { code: String },
    Reject { code: String },
}

#[derive(Debug, Clone, Subcommand)]
pub enum AppDevicesCommand {
    List,
    Revoke { device_id: String },
}

pub fn run_app_command(config_path: PathBuf, command: AppCommand) -> Result<()> {
    let config = Config::load_unvalidated(&config_path)?;
    let store = AppAuthStore::new(config.app_auth_file());

    match command {
        AppCommand::Pairing { command } => match command {
            AppPairingCommand::List => print_pairings(&store)?,
            AppPairingCommand::Approve { code } => {
                let (pairing, device) = store.approve_pairing_code(&code)?;
                println!(
                    "Approved APP pairing: code={} pairing_id={} device_id={} label={}",
                    pairing.code, pairing.pairing_id, device.device_id, device.label
                );
            }
            AppPairingCommand::Reject { code } => {
                let pairing = store.reject_pairing_code(&code)?;
                println!(
                    "Rejected APP pairing: code={} pairing_id={} label={}",
                    pairing.code, pairing.pairing_id, pairing.device_label
                );
            }
        },
        AppCommand::Devices { command } => match command {
            AppDevicesCommand::List => print_devices(&store)?,
            AppDevicesCommand::Revoke { device_id } => {
                let device = store.revoke_device(&device_id)?;
                println!(
                    "Revoked APP device: device_id={} label={} revoked_at={}",
                    device.device_id,
                    device.label,
                    device
                        .revoked_at
                        .expect("revoked_at must be set after revoke")
                );
            }
        },
    }

    Ok(())
}

fn print_pairings(store: &AppAuthStore) -> Result<()> {
    let pairings = store.list_pairings()?;
    if pairings.is_empty() {
        println!("No APP pairing requests.");
        return Ok(());
    }

    println!("APP pairing requests");
    println!();
    for pairing in pairings {
        println!(
            "- code={} pairing_id={} label={} status={:?} created_at={} expires_at={} claimed_at={}",
            pairing.code,
            pairing.pairing_id,
            pairing.device_label,
            pairing.status,
            pairing.created_at,
            pairing.expires_at,
            pairing
                .claimed_at
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".into())
        );
    }
    Ok(())
}

fn print_devices(store: &AppAuthStore) -> Result<()> {
    let devices = store.list_devices()?;
    if devices.is_empty() {
        println!("No APP devices.");
        return Ok(());
    }

    println!("APP devices");
    println!();
    for device in devices {
        println!(
            "- device_id={} label={} created_at={} last_seen_at={} revoked_at={}",
            device.device_id,
            device.label,
            device.created_at,
            device
                .last_seen_at
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".into()),
            device
                .revoked_at
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".into())
        );
    }
    Ok(())
}
