use std::path::PathBuf;

use anyhow::Result;
use clap::{Subcommand, ValueEnum};

use crate::config::Config;
use crate::state::StateStore;

#[derive(Debug, Clone, Subcommand)]
pub enum PairingCommand {
    List {
        #[arg(long, value_enum, default_value_t = PairingListMode::Pending)]
        mode: PairingListMode,
    },
    Approve {
        code: String,
    },
    Reject {
        code: String,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum PairingListMode {
    Pending,
    Approved,
    All,
}

pub fn run_pairing(config_path: PathBuf, command: PairingCommand) -> Result<()> {
    let config = Config::load_unvalidated(&config_path)?;
    let state_store = StateStore::new(config.state_file());
    let mut state = state_store.load()?;

    match command {
        PairingCommand::List { mode } => match mode {
            PairingListMode::Pending => print_pending(&state),
            PairingListMode::Approved => print_approved(&state),
            PairingListMode::All => {
                print_pending(&state);
                println!();
                print_approved(&state);
            }
        },
        PairingCommand::Approve { code } => {
            let approved = state.approve_pairing_code(&code)?;
            state_store.save(&state)?;
            println!(
                "Approved Telegram peer: user_id={} chat_id={} name={} username={}",
                approved.user_id,
                approved.chat_id,
                approved.first_name,
                approved.username.unwrap_or_else(|| "-".into())
            );
        }
        PairingCommand::Reject { code } => {
            let request = state.reject_pairing_code(&code)?;
            state_store.save(&state)?;
            println!(
                "Rejected pairing request: code={} user_id={} chat_id={}",
                request.code, request.user_id, request.chat_id
            );
        }
    }

    Ok(())
}

fn print_pending(state: &crate::state::AppState) {
    if state.list_pairing_requests().is_empty() {
        println!("No pending pairing requests.");
        return;
    }

    println!("Pending pairing requests");
    println!();
    for request in state.list_pairing_requests() {
        println!(
            "- code={} user_id={} chat_id={} name={} username={} created_at={}",
            request.code,
            request.user_id,
            request.chat_id,
            request.first_name,
            request.username.clone().unwrap_or_else(|| "-".into()),
            request.created_at
        );
    }
}

fn print_approved(state: &crate::state::AppState) {
    if state.list_approved_peers().is_empty() {
        println!("No approved Telegram peers.");
        return;
    }

    println!("Approved Telegram peers");
    println!();
    for peer in state.list_approved_peers() {
        println!(
            "- user_id={} chat_id={} name={} username={} approved_at={}",
            peer.user_id,
            peer.chat_id,
            peer.first_name,
            peer.username.clone().unwrap_or_else(|| "-".into()),
            peer.approved_at
        );
    }
}
