use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::config::{
    CodexConfig, Config, GitConfig, StateConfig, TelegramAccessMode, TelegramConfig, UiConfig,
    WorkspaceConfig,
};
use crate::telegram::api::{TelegramClient, default_bot_commands};

pub struct OnboardOptions {
    pub config_path: PathBuf,
    pub env_path: PathBuf,
    pub service_path: PathBuf,
}

pub async fn run(options: OnboardOptions) -> Result<()> {
    let mut config = if options.config_path.exists() {
        Config::load_unvalidated(&options.config_path)?
    } else {
        default_config()?
    };

    let mut env_map = load_env_file(&options.env_path)?;

    println!("MyCodex onboarding");
    println!();

    let bot_token = prompt_required(
        "Telegram bot token",
        if config.telegram.bot_token.trim().is_empty() {
            None
        } else {
            Some(config.telegram.bot_token.as_str())
        },
        true,
    )?;
    let telegram = TelegramClient::new(&bot_token);
    let me = telegram
        .get_me()
        .await
        .context("failed to validate Telegram bot token via getMe")?;
    println!(
        "Connected to Telegram bot: {}",
        me.username.unwrap_or(me.first_name)
    );
    match telegram.set_my_commands(&default_bot_commands()).await {
        Ok(()) => println!("Registered Telegram bot commands."),
        Err(err) => eprintln!("Warning: failed to register Telegram bot commands: {err}"),
    }

    let default_workspace = config.workspace.root.clone();
    let workspace_input = prompt_with_default(
        "Workspace path",
        &default_workspace.display().to_string(),
        false,
    )?;
    let workspace_root = expand_tilde(PathBuf::from(workspace_input));
    if !workspace_root.exists()
        && confirm(
            &format!(
                "Create workspace directory at {}?",
                workspace_root.display()
            ),
            true,
        )?
    {
        fs::create_dir_all(&workspace_root)
            .with_context(|| format!("failed to create {}", workspace_root.display()))?;
    }
    if !workspace_root.exists() {
        bail!(
            "workspace path does not exist: {}",
            workspace_root.display()
        );
    }

    let current_key = env_map.get("OPENAI_API_KEY").cloned();
    let api_key = prompt_optional("OpenAI API key", current_key.as_deref(), true)?;
    if let Some(key) = api_key {
        env_map.insert("OPENAI_API_KEY".to_string(), key);
    }

    config.workspace = WorkspaceConfig {
        root: workspace_root,
    };
    config.telegram = TelegramConfig {
        bot_token,
        access_mode: TelegramAccessMode::Pairing,
        allowed_user_id: None,
        allowed_chat_id: None,
        poll_timeout_seconds: config.telegram.poll_timeout_seconds,
    };

    write_config(&options.config_path, &config)?;
    write_env_file(&options.env_path, &env_map)?;

    run_self_check(&options.config_path, &env_map)?;

    if options.service_path.exists()
        && confirm(
            &format!(
                "A systemd unit exists at {}. Enable and start it now?",
                options.service_path.display()
            ),
            true,
        )?
    {
        let service_name = options
            .service_path
            .file_name()
            .and_then(|value| value.to_str())
            .context("service path must have a valid file name")?;
        match run_systemctl_enable_now(service_name) {
            Ok(()) => {
                println!("systemd service started: {service_name}");
            }
            Err(err) => {
                eprintln!("Failed to start service automatically: {err}");
                eprintln!("Run manually: sudo systemctl enable --now {service_name}");
            }
        }
    }

    println!();
    println!("Onboarding complete.");
    println!("Next steps:");
    println!("1. Send a message to your Telegram bot.");
    println!("2. Run `mycodex pairing list` on the server.");
    println!("3. Run `mycodex pairing approve <CODE>` to approve yourself.");
    Ok(())
}

fn default_config() -> Result<Config> {
    let home = std::env::var("HOME").context("HOME is not set")?;
    let home_path = PathBuf::from(home);
    Ok(Config {
        workspace: WorkspaceConfig {
            root: home_path.join("workspace"),
        },
        telegram: TelegramConfig {
            bot_token: String::new(),
            access_mode: TelegramAccessMode::Pairing,
            allowed_user_id: None,
            allowed_chat_id: None,
            poll_timeout_seconds: 30,
        },
        codex: CodexConfig {
            bin: "codex".into(),
            model: None,
        },
        state: StateConfig {
            dir: home_path.join(".local/state/mycodex"),
        },
        ui: UiConfig {
            stream_edit_interval_ms: 1_200,
            max_inline_diff_chars: 6_000,
        },
        git: GitConfig {
            clone_timeout_sec: 600,
            allow_ssh: true,
            allow_https: true,
        },
    })
}

fn load_env_file(path: &Path) -> Result<HashMap<String, String>> {
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read env file {}", path.display()))?;
    let mut env_map = HashMap::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = trimmed.split_once('=') {
            env_map.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    Ok(env_map)
}

fn write_env_file(path: &Path, env_map: &HashMap<String, String>) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut output = String::from("# MyCodex environment\n");
    if let Some(api_key) = env_map.get("OPENAI_API_KEY") {
        output.push_str(&format!("OPENAI_API_KEY={api_key}\n"));
    } else {
        output.push_str("# OPENAI_API_KEY=replace-me\n");
    }
    fs::write(path, output).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn write_config(path: &Path, config: &Config) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let raw = toml::to_string_pretty(config).context("failed to serialize config")?;
    fs::write(path, raw).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn run_self_check(config_path: &Path, env_map: &HashMap<String, String>) -> Result<()> {
    let current_exe = std::env::current_exe().context("failed to resolve current executable")?;
    let mut command = Command::new(current_exe);
    command.arg("check").arg("--config").arg(config_path);
    if let Some(api_key) = env_map.get("OPENAI_API_KEY") {
        command.env("OPENAI_API_KEY", api_key);
    }
    let status = command.status().context("failed to launch mycodex check")?;
    if !status.success() {
        bail!("`mycodex check` failed; update the configuration and retry onboarding");
    }
    Ok(())
}

fn run_systemctl_enable_now(service_name: &str) -> Result<()> {
    let mut command = if nix_like_root() {
        let mut cmd = Command::new("systemctl");
        cmd.arg("enable").arg("--now").arg(service_name);
        cmd
    } else {
        let mut cmd = Command::new("sudo");
        cmd.arg("systemctl")
            .arg("enable")
            .arg("--now")
            .arg(service_name);
        cmd
    };
    let status = command.status().context("failed to run systemctl")?;
    if !status.success() {
        bail!("systemctl exited with status {status}");
    }
    Ok(())
}

fn nix_like_root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim() == "0")
        .unwrap_or(false)
}

fn prompt_required(label: &str, default: Option<&str>, secret: bool) -> Result<String> {
    loop {
        let value = prompt(label, default, secret)?;
        if !value.trim().is_empty() {
            return Ok(value);
        }
    }
}

fn prompt_optional(label: &str, default: Option<&str>, secret: bool) -> Result<Option<String>> {
    let value = prompt(label, default, secret)?;
    if value.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(value))
    }
}

fn prompt_with_default(label: &str, default: &str, secret: bool) -> Result<String> {
    let value = prompt(label, Some(default), secret)?;
    if value.trim().is_empty() {
        Ok(default.to_string())
    } else {
        Ok(value)
    }
}

fn prompt(label: &str, default: Option<&str>, secret: bool) -> Result<String> {
    let mut stdout = io::stdout();
    match default {
        Some(default) if !secret => write!(stdout, "{label} [{default}]: ")?,
        Some(_) => write!(stdout, "{label} [configured]: ")?,
        None => write!(stdout, "{label}: ")?,
    }
    stdout.flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let value = input.trim().to_string();
    if value.is_empty() {
        Ok(default.unwrap_or_default().to_string())
    } else {
        Ok(value)
    }
}

fn confirm(label: &str, default_yes: bool) -> Result<bool> {
    let suffix = if default_yes { "[Y/n]" } else { "[y/N]" };
    let value = prompt(&format!("{label} {suffix}"), None, false)?;
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Ok(default_yes);
    }
    Ok(matches!(normalized.as_str(), "y" | "yes"))
}

fn expand_tilde(path: PathBuf) -> PathBuf {
    let value = path.to_string_lossy();
    if value == "~" || value.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            let suffix = value.trim_start_matches('~').trim_start_matches('/');
            return PathBuf::from(home).join(suffix);
        }
    }
    path
}
