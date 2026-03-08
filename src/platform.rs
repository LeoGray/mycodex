use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

const CONFIG_DIR_NAME: &str = "mycodex";

pub fn default_config_path() -> PathBuf {
    if cfg!(target_os = "linux") {
        PathBuf::from("/etc/mycodex/config.toml")
    } else {
        default_config_dir().join("config.toml")
    }
}

pub fn default_env_path() -> PathBuf {
    if cfg!(target_os = "linux") {
        PathBuf::from("/etc/mycodex/mycodex.env")
    } else {
        default_config_dir().join("mycodex.env")
    }
}

pub fn default_service_path() -> PathBuf {
    if cfg!(target_os = "linux") {
        PathBuf::from("/etc/systemd/system/mycodex.service")
    } else if cfg!(target_os = "macos") {
        user_home_dir()
            .join("Library/LaunchAgents")
            .join("com.leogray.mycodex.plist")
    } else {
        default_config_dir().join("mycodex.service")
    }
}

pub fn service_definition_name() -> &'static str {
    if cfg!(target_os = "linux") {
        "systemd unit"
    } else if cfg!(target_os = "macos") {
        "launchd agent"
    } else {
        "service definition"
    }
}

pub fn service_instance_name(service_path: &Path) -> Result<String> {
    service_name(service_path)
}

pub fn enable_and_start_service(service_path: &Path) -> Result<String> {
    if cfg!(target_os = "linux") {
        let service_name = service_name(service_path)?;
        run_systemctl_enable_now(&service_name)?;
        Ok(service_name)
    } else if cfg!(target_os = "macos") {
        let label = service_name(service_path)?;
        run_launchctl_load(service_path)?;
        Ok(label)
    } else {
        bail!("automatic service startup is unsupported on this platform");
    }
}

pub fn service_is_active(service_path: &Path) -> Result<bool> {
    if cfg!(target_os = "linux") {
        let service_name = service_name(service_path)?;
        let status = if nix_like_root() {
            let mut cmd = Command::new("systemctl");
            cmd.arg("is-active").arg("--quiet").arg(&service_name);
            cmd
        } else {
            let mut cmd = Command::new("sudo");
            cmd.arg("systemctl")
                .arg("is-active")
                .arg("--quiet")
                .arg(&service_name);
            cmd
        }
        .status()
        .context("failed to run systemctl is-active")?;
        Ok(status.success())
    } else if cfg!(target_os = "macos") {
        let service_name = service_name(service_path)?;
        let status = Command::new("launchctl")
            .arg("list")
            .arg(&service_name)
            .status()
            .context("failed to run launchctl list")?;
        Ok(status.success())
    } else {
        Ok(false)
    }
}

pub fn manual_start_hint(service_path: &Path) -> Result<String> {
    if cfg!(target_os = "linux") {
        Ok(format!(
            "sudo systemctl enable --now {}",
            service_name(service_path)?
        ))
    } else if cfg!(target_os = "macos") {
        Ok(format!("launchctl load -w {}", service_path.display()))
    } else {
        Ok(format!(
            "start the service defined at {} manually",
            service_path.display()
        ))
    }
}

fn default_config_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(path).join(CONFIG_DIR_NAME)
    } else {
        user_home_dir().join(".config").join(CONFIG_DIR_NAME)
    }
}

fn user_home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn service_name(service_path: &Path) -> Result<String> {
    if cfg!(target_os = "macos") {
        service_path
            .file_stem()
            .and_then(|value| value.to_str())
            .map(ToOwned::to_owned)
            .context("service path must have a valid plist file name")
    } else {
        service_path
            .file_name()
            .and_then(|value| value.to_str())
            .map(ToOwned::to_owned)
            .context("service path must have a valid file name")
    }
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

fn run_launchctl_load(service_path: &Path) -> Result<()> {
    let _ = Command::new("launchctl")
        .arg("unload")
        .arg(service_path)
        .status();

    let status = Command::new("launchctl")
        .arg("load")
        .arg("-w")
        .arg(service_path)
        .status()
        .context("failed to run launchctl")?;
    if !status.success() {
        bail!("launchctl exited with status {status}");
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
