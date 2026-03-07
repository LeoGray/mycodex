use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub workspace: WorkspaceConfig,
    pub telegram: TelegramConfig,
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
    pub allowed_user_id: i64,
    pub allowed_chat_id: Option<i64>,
    #[serde(default = "default_poll_timeout_seconds")]
    pub poll_timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexConfig {
    #[serde(default = "default_codex_bin")]
    pub bin: String,
    pub model: Option<String>,
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
    pub fn load(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        let config: Self = toml::from_str(&raw)
            .with_context(|| format!("failed to parse config file {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.telegram.bot_token.trim().is_empty() {
            bail!("telegram.bot_token must not be empty");
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
}

fn default_codex_bin() -> String {
    "codex".to_string()
}

fn default_poll_timeout_seconds() -> u64 {
    30
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
