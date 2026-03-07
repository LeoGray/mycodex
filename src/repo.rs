use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::process::Command;
use tokio::time::timeout;
use tracing::info;

use crate::config::Config;
use crate::state::{AppState, RepoRecord, normalize_path};

#[derive(Debug, Clone)]
pub struct DiscoveredRepo {
    pub name: String,
    pub path: PathBuf,
}

pub fn discover_workspace_repos(workspace_root: &Path) -> Result<Vec<DiscoveredRepo>> {
    let mut repos = Vec::new();
    for entry in std::fs::read_dir(workspace_root)
        .with_context(|| format!("failed to read workspace {}", workspace_root.display()))?
    {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if !file_type.is_dir() {
            continue;
        }

        let path = normalize_path(&entry.path());
        if path.join(".git").exists() {
            let name = entry.file_name().to_string_lossy().to_string();
            repos.push(DiscoveredRepo { name, path });
        }
    }
    repos.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(repos)
}

pub fn merge_discovered_repos(state: &mut AppState, discovered: Vec<DiscoveredRepo>) -> bool {
    let mut changed = false;
    let valid_paths: HashSet<PathBuf> = discovered.iter().map(|repo| repo.path.clone()).collect();

    for repo in discovered {
        let existed = state.repos.iter().any(|item| item.path == repo.path);
        let path = repo.path.clone();
        let entry = state.ensure_repo(repo.name, path, None);
        if !existed {
            changed = true;
        }
        if entry.path != repo.path {
            entry.path = repo.path;
            changed = true;
        }
    }

    let before = state.repos.len();
    state.remove_missing_repos(&valid_paths);
    changed || before != state.repos.len()
}

pub async fn clone_repo(
    config: &Config,
    git_url: &str,
    dir_name: Option<&str>,
    state: &mut AppState,
) -> Result<RepoRecord> {
    validate_git_url(git_url, config)?;

    let repo_name = match dir_name {
        Some(value) => sanitize_repo_name(value)?,
        None => derive_repo_name_from_url(git_url)?,
    };

    let target_path = config.workspace.root.join(&repo_name);
    if target_path.exists() {
        bail!(
            "target repo directory already exists: {}",
            target_path.display()
        );
    }

    let mut command = Command::new("git");
    command
        .arg("clone")
        .arg(git_url)
        .arg(&target_path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    info!("cloning repo {git_url} into {}", target_path.display());
    let timeout_dur = Duration::from_secs(config.git.clone_timeout_sec);
    let output = timeout(timeout_dur, command.output())
        .await
        .with_context(|| {
            format!(
                "git clone timed out after {}s",
                config.git.clone_timeout_sec
            )
        })?
        .context("failed to start git clone")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git clone failed: {}", stderr.trim());
    }

    let canonical = normalize_path(&target_path);
    let repo = state
        .ensure_repo(repo_name, canonical, Some(git_url.to_string()))
        .clone();
    Ok(repo)
}

pub fn validate_git_url(git_url: &str, config: &Config) -> Result<()> {
    if let Some(rest) = git_url.strip_prefix("https://") {
        if !config.git.allow_https {
            bail!("https git URLs are disabled");
        }
        if rest.is_empty() {
            bail!("git URL is missing host/path");
        }
        return Ok(());
    }
    if let Some(rest) = git_url.strip_prefix("ssh://") {
        if !config.git.allow_ssh {
            bail!("ssh git URLs are disabled");
        }
        if rest.is_empty() {
            bail!("git URL is missing host/path");
        }
        return Ok(());
    }
    if git_url.contains('@') && git_url.contains(':') {
        if !config.git.allow_ssh {
            bail!("scp-style ssh git URLs are disabled");
        }
        return Ok(());
    }
    bail!("unsupported git URL format: {git_url}");
}

pub fn derive_repo_name_from_url(git_url: &str) -> Result<String> {
    let tail = git_url
        .rsplit(['/', ':'])
        .next()
        .context("git URL must include a repo name")?;
    let raw = tail.strip_suffix(".git").unwrap_or(tail);
    sanitize_repo_name(raw)
}

fn sanitize_repo_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        bail!("repo name must not be empty");
    }
    if trimmed == "." || trimmed == ".." {
        bail!("repo name must not be '.' or '..'");
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        bail!("repo name must not contain path separators");
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        CodexConfig, GitConfig, StateConfig, TelegramConfig, UiConfig, WorkspaceConfig,
    };

    #[test]
    fn derives_repo_name_from_https_url() {
        let name = derive_repo_name_from_url("https://github.com/openai/codex.git").unwrap();
        assert_eq!(name, "codex");
    }

    #[test]
    fn derives_repo_name_from_scp_url() {
        let name = derive_repo_name_from_url("git@github.com:openai/codex.git").unwrap();
        assert_eq!(name, "codex");
    }

    #[test]
    fn validates_disabled_https() {
        let config = sample_config();
        let mut disabled = config.clone();
        disabled.git.allow_https = false;
        assert!(validate_git_url("https://github.com/openai/codex.git", &disabled).is_err());
    }

    fn sample_config() -> Config {
        Config {
            workspace: WorkspaceConfig {
                root: PathBuf::from("/tmp/workspace"),
            },
            telegram: TelegramConfig {
                bot_token: "token".into(),
                allowed_user_id: 1,
                allowed_chat_id: Some(1),
                poll_timeout_seconds: 30,
            },
            codex: CodexConfig {
                bin: "codex".into(),
                model: None,
            },
            state: StateConfig {
                dir: PathBuf::from("/tmp/state"),
            },
            ui: UiConfig {
                stream_edit_interval_ms: 1000,
                max_inline_diff_chars: 6000,
            },
            git: GitConfig {
                clone_timeout_sec: 30,
                allow_ssh: true,
                allow_https: true,
            },
        }
    }
}
