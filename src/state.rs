use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use uuid::Uuid;

use crate::codex::protocol::{FileChangeApprovalDecision, RpcId};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppState {
    pub active_repo_id: Option<String>,
    pub active_runtime_repo_id: Option<String>,
    pub active_turn_id: Option<String>,
    pub pending_request: Option<PendingRequest>,
    pub progress_message_id: Option<i64>,
    #[serde(default)]
    pub approval_rules: Vec<ApprovalRule>,
    #[serde(default)]
    pub approved_telegram_peers: Vec<ApprovedTelegramPeer>,
    #[serde(default)]
    pub pending_pairings: Vec<PairingRequest>,
    pub repos: Vec<RepoRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoRecord {
    pub repo_id: String,
    pub name: String,
    pub path: PathBuf,
    pub origin_url: Option<String>,
    pub active_thread_local_id: Option<String>,
    pub threads: Vec<ThreadRecord>,
    pub last_used_at: DateTime<Utc>,
}

impl RepoRecord {
    pub fn active_thread(&self) -> Option<&ThreadRecord> {
        let active_thread = self.active_thread_local_id.as_ref()?;
        self.threads
            .iter()
            .find(|thread| &thread.local_thread_id == active_thread)
    }

    pub fn active_thread_mut(&mut self) -> Option<&mut ThreadRecord> {
        let active_thread = self.active_thread_local_id.clone()?;
        self.threads
            .iter_mut()
            .find(|thread| thread.local_thread_id == active_thread)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadRecord {
    pub local_thread_id: String,
    pub codex_thread_id: String,
    #[serde(default)]
    pub codex_thread_path: Option<PathBuf>,
    pub repo_id: String,
    pub title: String,
    pub status: ThreadStatusRecord,
    pub created_at: DateTime<Utc>,
    pub last_used_at: DateTime<Utc>,
    #[serde(default)]
    pub has_user_message: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThreadStatusRecord {
    Active,
    Historical,
    Archived,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PendingRequest {
    CommandApproval {
        request_id: RpcId,
        repo_id: String,
        thread_local_id: String,
        thread_title: String,
        approval_chat_id: i64,
        approval_message_id: i64,
        approval_message_text: String,
        turn_id: String,
        item_id: String,
        command: Option<String>,
        cwd: Option<PathBuf>,
        reason: Option<String>,
    },
    FileApproval {
        request_id: RpcId,
        repo_id: String,
        thread_local_id: String,
        thread_title: String,
        approval_chat_id: i64,
        approval_message_id: i64,
        approval_message_text: String,
        turn_id: String,
        item_id: String,
        paths: Vec<String>,
        reason: Option<String>,
        diff_preview: String,
        patch_path: Option<PathBuf>,
        preferred_decision: FileChangeApprovalDecision,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalRule {
    pub rule_id: String,
    pub repo_id: String,
    pub command: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovedTelegramPeer {
    pub user_id: i64,
    pub chat_id: i64,
    pub first_name: String,
    pub username: Option<String>,
    pub approved_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairingRequest {
    pub code: String,
    pub user_id: i64,
    pub chat_id: i64,
    pub first_name: String,
    pub username: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub struct StateStore {
    path: PathBuf,
}

impl StateStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn load(&self) -> Result<AppState> {
        if !self.path.exists() {
            return Ok(AppState::default());
        }
        let raw = fs::read_to_string(&self.path)
            .with_context(|| format!("failed to read state file {}", self.path.display()))?;
        let state = serde_json::from_str::<AppState>(&raw)
            .with_context(|| format!("failed to parse state file {}", self.path.display()))?;
        Ok(state)
    }

    pub fn save(&self, state: &AppState) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create state dir {}", parent.display()))?;
        }

        let parent = self
            .path
            .parent()
            .context("state file path must have a parent directory")?;
        let mut tmp =
            NamedTempFile::new_in(parent).context("failed to create temporary state file")?;
        serde_json::to_writer_pretty(&mut tmp, state).context("failed to serialize state")?;
        tmp.persist(&self.path)
            .map_err(|err| err.error)
            .with_context(|| format!("failed to persist state to {}", self.path.display()))?;
        Ok(())
    }
}

impl AppState {
    pub fn clear_stale_runtime_state(&mut self) {
        self.active_runtime_repo_id = None;
        self.active_turn_id = None;
        self.pending_request = None;
        self.progress_message_id = None;
    }

    pub fn active_repo(&self) -> Option<&RepoRecord> {
        let active = self.active_repo_id.as_ref()?;
        self.repos.iter().find(|repo| &repo.repo_id == active)
    }

    pub fn active_repo_mut(&mut self) -> Option<&mut RepoRecord> {
        let active = self.active_repo_id.clone()?;
        self.repos.iter_mut().find(|repo| repo.repo_id == active)
    }

    pub fn find_repo_by_id(&self, repo_id: &str) -> Option<&RepoRecord> {
        self.repos.iter().find(|repo| repo.repo_id == repo_id)
    }

    pub fn find_repo_by_id_mut(&mut self, repo_id: &str) -> Option<&mut RepoRecord> {
        self.repos.iter_mut().find(|repo| repo.repo_id == repo_id)
    }

    pub fn resolve_repo_ref(&self, value: &str) -> Option<&RepoRecord> {
        if let Some(repo) = self.repos.iter().find(|repo| repo.name == value) {
            return Some(repo);
        }
        self.repos
            .iter()
            .find(|repo| repo.repo_id.starts_with(value))
    }

    pub fn set_active_repo(&mut self, repo_id: String) {
        self.active_repo_id = Some(repo_id);
    }

    pub fn ensure_repo(
        &mut self,
        name: String,
        path: PathBuf,
        origin_url: Option<String>,
    ) -> &mut RepoRecord {
        if let Some(index) = self.repos.iter().position(|repo| repo.path == path) {
            let repo = &mut self.repos[index];
            repo.name = name;
            if repo.origin_url.is_none() {
                repo.origin_url = origin_url;
            }
            return repo;
        }

        let repo = RepoRecord {
            repo_id: Uuid::new_v4().to_string(),
            name,
            path,
            origin_url,
            active_thread_local_id: None,
            threads: Vec::new(),
            last_used_at: Utc::now(),
        };
        self.repos.push(repo);
        self.repos
            .last_mut()
            .expect("repos must contain the repo we just pushed")
    }

    pub fn mark_repo_used(&mut self, repo_id: &str) {
        if let Some(repo) = self.find_repo_by_id_mut(repo_id) {
            repo.last_used_at = Utc::now();
        }
    }

    pub fn active_thread(&self) -> Option<&ThreadRecord> {
        self.active_repo()?.active_thread()
    }

    pub fn active_thread_mut(&mut self) -> Option<&mut ThreadRecord> {
        self.active_repo_mut()?.active_thread_mut()
    }

    pub fn is_peer_approved(&self, user_id: i64, chat_id: i64) -> bool {
        self.approved_telegram_peers
            .iter()
            .any(|peer| peer.user_id == user_id && peer.chat_id == chat_id)
    }

    pub fn ensure_pairing_request(
        &mut self,
        user_id: i64,
        chat_id: i64,
        first_name: String,
        username: Option<String>,
    ) -> PairingRequest {
        if let Some(existing) = self
            .pending_pairings
            .iter()
            .find(|request| request.user_id == user_id && request.chat_id == chat_id)
        {
            return existing.clone();
        }

        let request = PairingRequest {
            code: generate_pairing_code(),
            user_id,
            chat_id,
            first_name,
            username,
            created_at: Utc::now(),
        };
        self.pending_pairings.push(request.clone());
        request
    }

    pub fn list_pairing_requests(&self) -> &[PairingRequest] {
        &self.pending_pairings
    }

    pub fn list_approved_peers(&self) -> &[ApprovedTelegramPeer] {
        &self.approved_telegram_peers
    }

    pub fn approve_pairing_code(&mut self, code: &str) -> Result<ApprovedTelegramPeer> {
        let index = self
            .pending_pairings
            .iter()
            .position(|request| request.code.eq_ignore_ascii_case(code))
            .with_context(|| format!("pairing code not found: {code}"))?;
        let request = self.pending_pairings.remove(index);

        if let Some(existing) = self
            .approved_telegram_peers
            .iter()
            .find(|peer| peer.user_id == request.user_id && peer.chat_id == request.chat_id)
        {
            return Ok(existing.clone());
        }

        let approved = ApprovedTelegramPeer {
            user_id: request.user_id,
            chat_id: request.chat_id,
            first_name: request.first_name,
            username: request.username,
            approved_at: Utc::now(),
        };
        self.approved_telegram_peers.push(approved.clone());
        Ok(approved)
    }

    pub fn reject_pairing_code(&mut self, code: &str) -> Result<PairingRequest> {
        let index = self
            .pending_pairings
            .iter()
            .position(|request| request.code.eq_ignore_ascii_case(code))
            .with_context(|| format!("pairing code not found: {code}"))?;
        Ok(self.pending_pairings.remove(index))
    }

    pub fn resolve_thread_ref<'a>(
        &'a self,
        repo: &'a RepoRecord,
        value: &str,
    ) -> Option<&'a ThreadRecord> {
        if let Ok(index) = value.parse::<usize>() {
            if index > 0 {
                if let Some(thread) = repo.threads.get(index - 1) {
                    return Some(thread);
                }
            }
        }
        if let Some(thread) = repo
            .threads
            .iter()
            .find(|thread| thread.local_thread_id.starts_with(value))
        {
            return Some(thread);
        }
        if let Some(thread) = repo
            .threads
            .iter()
            .find(|thread| thread.codex_thread_id.starts_with(value))
        {
            return Some(thread);
        }
        repo.threads.iter().find(|thread| thread.title == value)
    }

    pub fn create_thread_for_repo(
        &mut self,
        repo_id: &str,
        codex_thread_id: String,
        codex_thread_path: Option<PathBuf>,
        title: String,
        has_user_message: bool,
    ) -> Result<ThreadRecord> {
        let now = Utc::now();
        let local_thread_id = Uuid::new_v4().to_string();
        let thread = ThreadRecord {
            local_thread_id: local_thread_id.clone(),
            codex_thread_id,
            codex_thread_path,
            repo_id: repo_id.to_string(),
            title,
            status: ThreadStatusRecord::Active,
            created_at: now,
            last_used_at: now,
            has_user_message,
        };

        let repo = self
            .find_repo_by_id_mut(repo_id)
            .with_context(|| format!("repo not found: {repo_id}"))?;

        for item in &mut repo.threads {
            if item.status == ThreadStatusRecord::Active {
                item.status = ThreadStatusRecord::Historical;
            }
        }

        repo.active_thread_local_id = Some(local_thread_id);
        repo.last_used_at = now;
        repo.threads.push(thread.clone());
        Ok(thread)
    }

    pub fn update_thread_runtime_metadata(
        &mut self,
        repo_id: &str,
        local_thread_id: &str,
        codex_thread_id: String,
        codex_thread_path: Option<PathBuf>,
    ) -> Result<()> {
        let repo = self
            .find_repo_by_id_mut(repo_id)
            .with_context(|| format!("repo not found: {repo_id}"))?;
        let thread = repo
            .threads
            .iter_mut()
            .find(|thread| thread.local_thread_id == local_thread_id)
            .with_context(|| format!("thread not found: {local_thread_id}"))?;
        thread.codex_thread_id = codex_thread_id;
        if codex_thread_path.is_some() {
            thread.codex_thread_path = codex_thread_path;
        }
        thread.last_used_at = Utc::now();
        Ok(())
    }

    pub fn clear_active_thread(&mut self, repo_id: &str) -> Result<Option<ThreadRecord>> {
        let repo = self
            .find_repo_by_id_mut(repo_id)
            .with_context(|| format!("repo not found: {repo_id}"))?;
        let active_thread_id = match repo.active_thread_local_id.take() {
            Some(active_thread_id) => active_thread_id,
            None => return Ok(None),
        };
        let thread = repo
            .threads
            .iter_mut()
            .find(|thread| thread.local_thread_id == active_thread_id);
        if let Some(thread) = thread {
            if thread.status == ThreadStatusRecord::Active {
                thread.status = ThreadStatusRecord::Historical;
            }
            return Ok(Some(thread.clone()));
        }
        Ok(None)
    }

    pub fn activate_thread(&mut self, repo_id: &str, local_thread_id: &str) -> Result<()> {
        let repo = self
            .find_repo_by_id_mut(repo_id)
            .with_context(|| format!("repo not found: {repo_id}"))?;
        let mut found = false;
        for thread in &mut repo.threads {
            if thread.local_thread_id == local_thread_id {
                thread.status = ThreadStatusRecord::Active;
                thread.last_used_at = Utc::now();
                found = true;
            } else if thread.status == ThreadStatusRecord::Active {
                thread.status = ThreadStatusRecord::Historical;
            }
        }
        if !found {
            anyhow::bail!("thread not found: {local_thread_id}");
        }
        repo.active_thread_local_id = Some(local_thread_id.to_string());
        repo.last_used_at = Utc::now();
        Ok(())
    }

    pub fn update_active_thread_title(&mut self, title: String) {
        if let Some(thread) = self.active_thread_mut() {
            thread.title = title;
            thread.has_user_message = true;
            thread.last_used_at = Utc::now();
        }
    }

    pub fn approval_rules_for_repo(&self, repo_id: &str) -> Vec<&ApprovalRule> {
        self.approval_rules
            .iter()
            .filter(|rule| rule.repo_id == repo_id)
            .collect()
    }

    pub fn find_matching_approval_rule(
        &self,
        repo_id: &str,
        command: &str,
    ) -> Option<&ApprovalRule> {
        self.approval_rules
            .iter()
            .find(|rule| rule.repo_id == repo_id && rule.command == command)
    }

    pub fn add_approval_rule(&mut self, repo_id: &str, command: String) -> ApprovalRule {
        if let Some(rule) = self.find_matching_approval_rule(repo_id, &command) {
            return rule.clone();
        }

        let rule = ApprovalRule {
            rule_id: Uuid::new_v4().to_string(),
            repo_id: repo_id.to_string(),
            command,
            created_at: Utc::now(),
        };
        self.approval_rules.push(rule.clone());
        rule
    }

    pub fn remove_approval_rule(&mut self, repo_id: &str, value: &str) -> Result<ApprovalRule> {
        let index = self
            .approval_rules
            .iter()
            .enumerate()
            .filter(|(_, rule)| rule.repo_id == repo_id)
            .enumerate()
            .find_map(|(index, rule)| {
                let (global_index, rule) = rule;
                if approval_rule_matches(value, rule, index) {
                    Some(global_index)
                } else {
                    None
                }
            })
            .with_context(|| format!("approval rule not found: {value}"))?;
        Ok(self.approval_rules.remove(index))
    }

    pub fn clear_approval_rules(&mut self, repo_id: &str) -> usize {
        let before = self.approval_rules.len();
        self.approval_rules.retain(|rule| rule.repo_id != repo_id);
        before - self.approval_rules.len()
    }

    pub fn remove_missing_repos(&mut self, valid_paths: &HashSet<PathBuf>) {
        let active_path = self.active_repo().map(|repo| repo.path.clone());
        self.repos.retain(|repo| valid_paths.contains(&repo.path));
        if let Some(active_path) = active_path {
            if !valid_paths.contains(&active_path) {
                self.active_repo_id = None;
                self.active_runtime_repo_id = None;
            }
        }
    }
}

pub fn normalize_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn generate_pairing_code() -> String {
    Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(8)
        .collect::<String>()
        .to_uppercase()
}

fn approval_rule_matches(value: &str, rule: &ApprovalRule, repo_rule_index: usize) -> bool {
    if let Ok(index) = value.parse::<usize>() {
        return index > 0 && repo_rule_index + 1 == index;
    }
    rule.rule_id.starts_with(value) || rule.command == value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_resolution_supports_index() {
        let repo = RepoRecord {
            repo_id: "repo-1".into(),
            name: "alpha".into(),
            path: PathBuf::from("/tmp/alpha"),
            origin_url: None,
            active_thread_local_id: Some("t-2".into()),
            threads: vec![
                ThreadRecord {
                    local_thread_id: "t-1".into(),
                    codex_thread_id: "thr-1".into(),
                    codex_thread_path: None,
                    repo_id: "repo-1".into(),
                    title: "first".into(),
                    status: ThreadStatusRecord::Historical,
                    created_at: Utc::now(),
                    last_used_at: Utc::now(),
                    has_user_message: true,
                },
                ThreadRecord {
                    local_thread_id: "t-2".into(),
                    codex_thread_id: "thr-2".into(),
                    codex_thread_path: None,
                    repo_id: "repo-1".into(),
                    title: "second".into(),
                    status: ThreadStatusRecord::Active,
                    created_at: Utc::now(),
                    last_used_at: Utc::now(),
                    has_user_message: true,
                },
            ],
            last_used_at: Utc::now(),
        };
        let state = AppState {
            repos: vec![repo.clone()],
            active_repo_id: Some("repo-1".into()),
            ..AppState::default()
        };

        let resolved = state.resolve_thread_ref(&repo, "2").unwrap();
        assert_eq!(resolved.local_thread_id, "t-2");
    }

    #[test]
    fn thread_resolution_falls_back_to_local_id_prefix_for_numeric_values() {
        let repo = RepoRecord {
            repo_id: "repo-1".into(),
            name: "alpha".into(),
            path: PathBuf::from("/tmp/alpha"),
            origin_url: None,
            active_thread_local_id: Some("81764991-54e9-42e7-bad6-e1625025156e".into()),
            threads: vec![ThreadRecord {
                local_thread_id: "81764991-54e9-42e7-bad6-e1625025156e".into(),
                codex_thread_id: "019cc8e5-d151-7811-834f-a6eaa5e2ede8".into(),
                codex_thread_path: None,
                repo_id: "repo-1".into(),
                title: "hello".into(),
                status: ThreadStatusRecord::Active,
                created_at: Utc::now(),
                last_used_at: Utc::now(),
                has_user_message: true,
            }],
            last_used_at: Utc::now(),
        };
        let state = AppState::default();

        let resolved = state.resolve_thread_ref(&repo, "81764991").unwrap();
        assert_eq!(
            resolved.local_thread_id,
            "81764991-54e9-42e7-bad6-e1625025156e"
        );
    }

    #[test]
    fn repo_active_thread_resolves_from_active_thread_local_id() {
        let repo = RepoRecord {
            repo_id: "repo-1".into(),
            name: "alpha".into(),
            path: PathBuf::from("/tmp/alpha"),
            origin_url: None,
            active_thread_local_id: Some("t-2".into()),
            threads: vec![
                ThreadRecord {
                    local_thread_id: "t-1".into(),
                    codex_thread_id: "thr-1".into(),
                    codex_thread_path: None,
                    repo_id: "repo-1".into(),
                    title: "first".into(),
                    status: ThreadStatusRecord::Historical,
                    created_at: Utc::now(),
                    last_used_at: Utc::now(),
                    has_user_message: true,
                },
                ThreadRecord {
                    local_thread_id: "t-2".into(),
                    codex_thread_id: "thr-2".into(),
                    codex_thread_path: None,
                    repo_id: "repo-1".into(),
                    title: "second".into(),
                    status: ThreadStatusRecord::Active,
                    created_at: Utc::now(),
                    last_used_at: Utc::now(),
                    has_user_message: true,
                },
            ],
            last_used_at: Utc::now(),
        };

        let resolved = repo.active_thread().unwrap();
        assert_eq!(resolved.local_thread_id, "t-2");
        assert_eq!(resolved.codex_thread_id, "thr-2");
    }

    #[test]
    fn stale_runtime_state_is_cleared() {
        let mut state = AppState {
            active_runtime_repo_id: Some("repo-1".into()),
            active_turn_id: Some("turn-1".into()),
            pending_request: Some(PendingRequest::CommandApproval {
                request_id: RpcId::Number(1),
                repo_id: "repo-1".into(),
                thread_local_id: "thread-1".into(),
                thread_title: "demo".into(),
                approval_chat_id: 10,
                approval_message_id: 99,
                approval_message_text: "approve git status".into(),
                turn_id: "turn-1".into(),
                item_id: "item-1".into(),
                command: Some("git status".into()),
                cwd: Some(PathBuf::from("/tmp/demo")),
                reason: None,
            }),
            progress_message_id: Some(99),
            ..AppState::default()
        };

        state.clear_stale_runtime_state();
        assert!(state.active_runtime_repo_id.is_none());
        assert!(state.active_turn_id.is_none());
        assert!(state.pending_request.is_none());
        assert!(state.progress_message_id.is_none());
    }

    #[test]
    fn clear_active_thread_unsets_repo_active_thread() {
        let mut state = AppState {
            repos: vec![RepoRecord {
                repo_id: "repo-1".into(),
                name: "alpha".into(),
                path: PathBuf::from("/tmp/alpha"),
                origin_url: None,
                active_thread_local_id: Some("t-2".into()),
                threads: vec![
                    ThreadRecord {
                        local_thread_id: "t-1".into(),
                        codex_thread_id: "thr-1".into(),
                        codex_thread_path: None,
                        repo_id: "repo-1".into(),
                        title: "first".into(),
                        status: ThreadStatusRecord::Historical,
                        created_at: Utc::now(),
                        last_used_at: Utc::now(),
                        has_user_message: true,
                    },
                    ThreadRecord {
                        local_thread_id: "t-2".into(),
                        codex_thread_id: "thr-2".into(),
                        codex_thread_path: None,
                        repo_id: "repo-1".into(),
                        title: "second".into(),
                        status: ThreadStatusRecord::Active,
                        created_at: Utc::now(),
                        last_used_at: Utc::now(),
                        has_user_message: true,
                    },
                ],
                last_used_at: Utc::now(),
            }],
            ..AppState::default()
        };

        let cleared = state.clear_active_thread("repo-1").unwrap().unwrap();

        assert_eq!(cleared.local_thread_id, "t-2");
        assert!(state.repos[0].active_thread_local_id.is_none());
        assert_eq!(
            state.repos[0].threads[1].status,
            ThreadStatusRecord::Historical
        );
    }

    #[test]
    fn approval_rule_roundtrip() {
        let mut state = AppState::default();
        let rule = state.add_approval_rule("repo-1", "git status".into());

        let matched = state
            .find_matching_approval_rule("repo-1", "git status")
            .unwrap();
        assert_eq!(matched.rule_id, rule.rule_id);

        let removed = state.remove_approval_rule("repo-1", "1").unwrap();
        assert_eq!(removed.rule_id, rule.rule_id);
        assert!(state.approval_rules.is_empty());
    }

    #[test]
    fn clear_approval_rules_only_removes_rules_for_target_repo() {
        let mut state = AppState::default();
        state.add_approval_rule("repo-1", "git status".into());
        state.add_approval_rule("repo-2", "git fetch origin".into());

        let removed = state.clear_approval_rules("repo-1");

        assert_eq!(removed, 1);
        assert_eq!(state.approval_rules.len(), 1);
        assert_eq!(state.approval_rules[0].repo_id, "repo-2");
    }

    #[test]
    fn approval_rule_remove_uses_repo_local_index() {
        let mut state = AppState::default();
        state.add_approval_rule("repo-2", "git fetch origin".into());
        let target = state.add_approval_rule("repo-1", "git status".into());

        let removed = state.remove_approval_rule("repo-1", "1").unwrap();

        assert_eq!(removed.rule_id, target.rule_id);
        assert_eq!(state.approval_rules.len(), 1);
        assert_eq!(state.approval_rules[0].repo_id, "repo-2");
    }

    #[test]
    fn pairing_request_roundtrip() {
        let mut state = AppState::default();
        let request = state.ensure_pairing_request(1, 10, "Leo".into(), Some("leogray".into()));
        assert_eq!(state.list_pairing_requests().len(), 1);
        let approved = state.approve_pairing_code(&request.code).unwrap();
        assert_eq!(approved.user_id, 1);
        assert!(state.is_peer_approved(1, 10));
        assert!(state.list_pairing_requests().is_empty());
    }
}
