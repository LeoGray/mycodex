use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub workspace: WorkspaceConfig,
    pub telegram: TelegramConfig,
    #[serde(default)]
    pub app: AppConfig,
    pub codex: CodexConfig,
    pub state: StateConfig,
    pub ui: UiConfig,
    pub git: GitConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    pub root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    pub bot_token: String,
    #[serde(default = "default_telegram_access_mode")]
    pub access_mode: TelegramAccessMode,
    pub allowed_user_id: Option<i64>,
    pub allowed_chat_id: Option<i64>,
    #[serde(default = "default_poll_timeout_seconds")]
    pub poll_timeout_seconds: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TelegramAccessMode {
    Pairing,
    StaticAllowlist,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexConfig {
    #[serde(default = "default_codex_bin")]
    pub bin: String,
    pub model: Option<String>,
    #[serde(default = "default_network_access")]
    pub network_access: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_app_bind_addr")]
    pub bind_addr: String,
    #[serde(default)]
    pub public_base_url: String,
    #[serde(default = "default_app_pairing_code_ttl_sec")]
    pub pairing_code_ttl_sec: u64,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_addr: default_app_bind_addr(),
            public_base_url: String::new(),
            pairing_code_ttl_sec: default_app_pairing_code_ttl_sec(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateConfig {
    pub dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    #[serde(default = "default_stream_edit_interval_ms")]
    pub stream_edit_interval_ms: u64,
    #[serde(default = "default_max_inline_diff_chars")]
    pub max_inline_diff_chars: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitConfig {
    #[serde(default = "default_clone_timeout_sec")]
    pub clone_timeout_sec: u64,
    #[serde(default = "default_allow_ssh")]
    pub allow_ssh: bool,
    #[serde(default = "default_allow_https")]
    pub allow_https: bool,
}

impl Config {
    pub fn load_unvalidated(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        let config: Self = toml::from_str(&raw)
            .with_context(|| format!("failed to parse config file {}", path.display()))?;
        Ok(config)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let config = Self::load_unvalidated(path)?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.telegram.is_enabled()
            && self.telegram.access_mode == TelegramAccessMode::StaticAllowlist
            && self.telegram.allowed_user_id.is_none()
        {
            bail!(
                "telegram.allowed_user_id is required when telegram.access_mode = \"static_allowlist\""
            );
        }
        if !self.workspace.root.is_absolute() {
            bail!("workspace.root must be an absolute path");
        }
        if !self.state.dir.is_absolute() {
            bail!("state.dir must be an absolute path");
        }
        if !self.workspace.root.exists() {
            bail!(
                "workspace.root does not exist: {}",
                self.workspace.root.display()
            );
        }
        if !self.workspace.root.is_dir() {
            bail!(
                "workspace.root is not a directory: {}",
                self.workspace.root.display()
            );
        }
        if self.codex.bin.trim().is_empty() {
            bail!("codex.bin must not be empty");
        }
        if self.app.bind_addr.trim().is_empty() {
            bail!("app.bind_addr must not be empty");
        }
        self.app
            .bind_addr
            .parse::<std::net::SocketAddr>()
            .with_context(|| {
                format!(
                    "app.bind_addr must be a valid socket address, got {}",
                    self.app.bind_addr
                )
            })?;
        if self.app.pairing_code_ttl_sec == 0 {
            bail!("app.pairing_code_ttl_sec must be greater than 0");
        }
        if self.ui.stream_edit_interval_ms == 0 {
            bail!("ui.stream_edit_interval_ms must be greater than 0");
        }
        if self.ui.max_inline_diff_chars < 200 {
            bail!("ui.max_inline_diff_chars must be at least 200");
        }
        if self.git.clone_timeout_sec == 0 {
            bail!("git.clone_timeout_sec must be greater than 0");
        }
        if !self.git.allow_https && !self.git.allow_ssh {
            bail!("at least one of git.allow_https or git.allow_ssh must be enabled");
        }
        Ok(())
    }

    pub fn state_file(&self) -> PathBuf {
        self.state.dir.join("state.json")
    }

    pub fn temp_dir(&self) -> PathBuf {
        self.state.dir.join("tmp")
    }

    pub fn app_auth_file(&self) -> PathBuf {
        self.state.dir.join("app_auth.json")
    }
}

impl TelegramAccessMode {
    pub fn is_pairing(self) -> bool {
        self == Self::Pairing
    }
}

impl TelegramConfig {
    pub fn is_enabled(&self) -> bool {
        !self.bot_token.trim().is_empty()
    }
}

fn default_codex_bin() -> String {
    "codex".to_string()
}

fn default_telegram_access_mode() -> TelegramAccessMode {
    TelegramAccessMode::Pairing
}

fn default_poll_timeout_seconds() -> u64 {
    30
}

fn default_network_access() -> bool {
    true
}

fn default_app_bind_addr() -> String {
    "127.0.0.1:3940".to_string()
}

fn default_app_pairing_code_ttl_sec() -> u64 {
    600
}

fn default_stream_edit_interval_ms() -> u64 {
    1_200
}

fn default_max_inline_diff_chars() -> usize {
    6_000
}

fn default_clone_timeout_sec() -> u64 {
    600
}

fn default_allow_ssh() -> bool {
    true
}

fn default_allow_https() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn valid_config() -> Config {
        let workspace = tempdir().unwrap();
        let state = tempdir().unwrap();

        Config {
            workspace: WorkspaceConfig {
                root: workspace.keep(),
            },
            telegram: TelegramConfig {
                bot_token: String::new(),
                access_mode: TelegramAccessMode::Pairing,
                allowed_user_id: None,
                allowed_chat_id: None,
                poll_timeout_seconds: 30,
            },
            app: AppConfig {
                enabled: true,
                bind_addr: "127.0.0.1:3940".into(),
                public_base_url: String::new(),
                pairing_code_ttl_sec: 600,
            },
            codex: CodexConfig {
                bin: "codex".into(),
                model: None,
                network_access: true,
            },
            state: StateConfig { dir: state.keep() },
            ui: UiConfig {
                stream_edit_interval_ms: 1_200,
                max_inline_diff_chars: 6_000,
            },
            git: GitConfig {
                clone_timeout_sec: 600,
                allow_ssh: true,
                allow_https: true,
            },
        }
    }

    #[test]
    fn validate_allows_empty_telegram_token() {
        valid_config().validate().unwrap();
    }

    #[test]
    fn validate_requires_allowlist_user_when_telegram_enabled() {
        let mut config = valid_config();
        config.telegram.bot_token = "bot-token".into();
        config.telegram.access_mode = TelegramAccessMode::StaticAllowlist;

        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("telegram.allowed_user_id is required"));
    }
}
