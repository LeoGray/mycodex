use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::Utc;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tracing::{info, warn};

use crate::codex::protocol::{
    CommandExecutionApprovalDecision, FileChangeApprovalDecision, FileUpdateChange, RpcId,
    ThreadItem, TurnStatus,
};
use crate::codex::runtime::{CodexEvent, CodexRuntime};
use crate::commands::{
    ApprovalCommand, Command, RepoCommand, ThreadCommand, UserInput, parse_user_input,
};
use crate::config::{Config, TelegramAccessMode};
use crate::repo::{clone_repo, discover_workspace_repos, merge_discovered_repos};
use crate::state::{AppState, PendingRequest, StateStore, ThreadRecord};
use crate::telegram::api::{
    CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, TelegramClient, TelegramMessage,
    Update, default_bot_commands,
};
use crate::telegram::render::{
    ProgressView, render_approval_rules, render_command_approval, render_file_approval,
    render_help, render_progress, render_repo_list, render_repo_status, render_status,
    render_thread_list, render_thread_status, short_id, split_message, title_from_text,
};

pub struct App {
    config: Config,
    telegram: TelegramClient,
    state_store: StateStore,
    state: AppState,
    runtime: Option<CodexRuntime>,
    codex_events_tx: mpsc::Sender<CodexEvent>,
    codex_events_rx: mpsc::Receiver<CodexEvent>,
    update_offset: i64,
    progress: Option<ActiveProgress>,
    active_file_changes: HashMap<String, Vec<FileUpdateChange>>,
}

#[derive(Debug, Clone)]
struct ActiveProgress {
    chat_id: i64,
    repo_id: String,
    thread_local_id: String,
    message_id: i64,
    assistant_text: String,
    command_output_tail: String,
    diff_preview: String,
    last_rendered_at: Option<Instant>,
}

impl App {
    pub async fn new(config: Config) -> Result<Self> {
        tokio::fs::create_dir_all(&config.state.dir)
            .await
            .with_context(|| format!("failed to create {}", config.state.dir.display()))?;
        tokio::fs::create_dir_all(config.temp_dir())
            .await
            .with_context(|| format!("failed to create {}", config.temp_dir().display()))?;

        let state_store = StateStore::new(config.state_file());
        let mut state = state_store.load()?;
        state.clear_stale_runtime_state();
        let discovered = discover_workspace_repos(&config.workspace.root)?;
        let _changed = merge_discovered_repos(&mut state, discovered);
        state_store.save(&state)?;

        let telegram = TelegramClient::new(&config.telegram.bot_token);
        if let Err(err) = telegram.set_my_commands(&default_bot_commands()).await {
            warn!("failed to register Telegram bot commands: {err}");
        }
        let (codex_events_tx, codex_events_rx) = mpsc::channel(256);

        let mut app = Self {
            config,
            telegram,
            state_store,
            state,
            runtime: None,
            codex_events_tx,
            codex_events_rx,
            update_offset: 0,
            progress: None,
            active_file_changes: HashMap::new(),
        };

        if let Some(active_repo_id) = app.state.active_repo_id.clone() {
            if let Err(err) = app.ensure_runtime_for_repo(&active_repo_id).await {
                warn!(
                    "failed to restore active repo runtime {}: {err}",
                    active_repo_id
                );
            }
        }

        Ok(app)
    }

    pub async fn check(config: Config) -> Result<()> {
        tokio::fs::create_dir_all(&config.state.dir)
            .await
            .with_context(|| format!("failed to create {}", config.state.dir.display()))?;
        tokio::fs::create_dir_all(config.temp_dir())
            .await
            .with_context(|| format!("failed to create {}", config.temp_dir().display()))?;

        let telegram = TelegramClient::new(&config.telegram.bot_token);
        let user = telegram.get_me().await.context("telegram getMe failed")?;
        info!(
            "telegram token valid for bot {}",
            user.username.unwrap_or(user.first_name)
        );

        probe_codex(&config).await?;
        let discovered = discover_workspace_repos(&config.workspace.root)?;
        info!("workspace scan found {} repo(s)", discovered.len());
        Ok(())
    }

    pub async fn run(&mut self) -> Result<()> {
        info!("starting MyCodex event loop");
        loop {
            tokio::select! {
                result = self.telegram.get_updates(self.update_offset, self.config.telegram.poll_timeout_seconds) => {
                    match result {
                        Ok(updates) => {
                            for update in updates {
                                self.update_offset = update.update_id + 1;
                                if let Err(err) = self.handle_update(update).await {
                                    warn!("failed to handle telegram update: {err}");
                                }
                            }
                        }
                        Err(err) => {
                            warn!("telegram polling failed: {err}");
                            sleep(Duration::from_secs(2)).await;
                        }
                    }
                }
                maybe_event = self.codex_events_rx.recv() => {
                    match maybe_event {
                        Some(event) => {
                            if let Err(err) = self.handle_codex_event(event).await {
                                warn!("failed to handle codex event: {err}");
                            }
                        }
                        None => break,
                    }
                }
            }
        }

        Ok(())
    }

    async fn handle_update(&mut self, update: Update) -> Result<()> {
        self.reload_pairing_state().ok();
        let chat_id = update
            .message
            .as_ref()
            .map(|message| message.chat.id)
            .or_else(|| {
                update
                    .callback_query
                    .as_ref()
                    .and_then(|callback| callback.message.as_ref().map(|message| message.chat.id))
            });
        self.reconcile_runtime_exit(chat_id).await?;

        if let Some(message) = update.message {
            self.handle_incoming_message(message).await?;
            return Ok(());
        }

        if let Some(callback) = update.callback_query {
            self.handle_incoming_callback(callback).await?;
        }

        Ok(())
    }

    async fn handle_incoming_message(&mut self, message: TelegramMessage) -> Result<()> {
        match self.message_access(&message) {
            MessageAccess::Allowed => {
                if let Some(text) = message.text.clone() {
                    self.handle_message(message.chat.id, text).await?;
                }
            }
            MessageAccess::NeedsPairing => {
                self.handle_pairing_message(&message).await?;
            }
            MessageAccess::Denied => {}
        }
        Ok(())
    }

    async fn handle_incoming_callback(&mut self, callback: CallbackQuery) -> Result<()> {
        if !self.is_allowed_callback(&callback) {
            return Ok(());
        }
        self.handle_callback(callback).await
    }

    fn message_access(&self, message: &TelegramMessage) -> MessageAccess {
        let from = match &message.from {
            Some(from) => from,
            None => return MessageAccess::Denied,
        };
        match self.config.telegram.access_mode {
            TelegramAccessMode::StaticAllowlist => {
                if self.config.telegram.allowed_user_id != Some(from.id) {
                    return MessageAccess::Denied;
                }
                if let Some(chat_id) = self.config.telegram.allowed_chat_id {
                    if message.chat.id != chat_id {
                        return MessageAccess::Denied;
                    }
                }
                MessageAccess::Allowed
            }
            TelegramAccessMode::Pairing => {
                if self.state.is_peer_approved(from.id, message.chat.id) {
                    MessageAccess::Allowed
                } else {
                    MessageAccess::NeedsPairing
                }
            }
        }
    }

    fn is_allowed_callback(&self, callback: &CallbackQuery) -> bool {
        match self.config.telegram.access_mode {
            TelegramAccessMode::StaticAllowlist => {
                if self.config.telegram.allowed_user_id != Some(callback.from.id) {
                    return false;
                }
                if let Some(chat_id) = self.config.telegram.allowed_chat_id {
                    let callback_chat = callback.message.as_ref().map(|message| message.chat.id);
                    if callback_chat != Some(chat_id) {
                        return false;
                    }
                }
                true
            }
            TelegramAccessMode::Pairing => callback
                .message
                .as_ref()
                .map(|message| {
                    self.state
                        .is_peer_approved(callback.from.id, message.chat.id)
                })
                .unwrap_or(false),
        }
    }

    async fn handle_message(&mut self, chat_id: i64, text: String) -> Result<()> {
        match parse_user_input(&text) {
            UserInput::Command(command) => self.handle_command(chat_id, command).await,
            UserInput::Text(text) => self.handle_text(chat_id, text).await,
        }
    }

    async fn handle_command(&mut self, chat_id: i64, command: Command) -> Result<()> {
        match command {
            Command::Start => {
                self.telegram
                    .send_message(chat_id, &render_help(), None)
                    .await?;
            }
            Command::Help => {
                self.telegram
                    .send_message(chat_id, &render_help(), None)
                    .await?;
            }
            Command::Status => {
                let approval_rule_count = self
                    .state
                    .active_repo_id
                    .as_deref()
                    .map(|repo_id| self.state.approval_rules_for_repo(repo_id).len())
                    .unwrap_or(0);
                let body = render_status(
                    self.state.active_repo(),
                    self.state.active_thread(),
                    self.state.active_runtime_repo_id.as_deref(),
                    self.state.active_turn_id.as_deref(),
                    self.state.pending_request.as_ref(),
                    approval_rule_count,
                );
                self.telegram.send_message(chat_id, &body, None).await?;
            }
            Command::Abort => {
                self.abort_active_turn(chat_id).await?;
            }
            Command::Approval(command) => {
                self.handle_approval_command(chat_id, command).await?;
            }
            Command::Repo(command) => {
                self.handle_repo_command(chat_id, command).await?;
            }
            Command::Thread(command) => {
                self.handle_thread_command(chat_id, command).await?;
            }
        }
        Ok(())
    }

    async fn handle_approval_command(
        &mut self,
        chat_id: i64,
        command: ApprovalCommand,
    ) -> Result<()> {
        let active_repo = match self.state.active_repo() {
            Some(repo) => repo.clone(),
            None => {
                self.telegram
                    .send_message(
                        chat_id,
                        "No active repo. Use /repo use or /repo clone first.",
                        None,
                    )
                    .await?;
                return Ok(());
            }
        };

        match command {
            ApprovalCommand::List => {
                let rules = self.state.approval_rules_for_repo(&active_repo.repo_id);
                let body = render_approval_rules(&active_repo, &rules);
                self.telegram.send_message(chat_id, &body, None).await?;
            }
            ApprovalCommand::Remove { rule } => {
                let removed = self
                    .state
                    .remove_approval_rule(&active_repo.repo_id, &rule)?;
                self.persist_state()?;
                self.telegram
                    .send_message(
                        chat_id,
                        &format!(
                            "Removed approval rule [{}] for command:\n{}",
                            short_id(&removed.rule_id),
                            removed.command
                        ),
                        None,
                    )
                    .await?;
            }
            ApprovalCommand::Clear => {
                let removed = self.state.clear_approval_rules(&active_repo.repo_id);
                self.persist_state()?;
                self.telegram
                    .send_message(
                        chat_id,
                        &format!(
                            "Cleared {} approval rule(s) for repo {}.",
                            removed, active_repo.name
                        ),
                        None,
                    )
                    .await?;
            }
        }
        Ok(())
    }

    async fn handle_repo_command(&mut self, chat_id: i64, command: RepoCommand) -> Result<()> {
        match command {
            RepoCommand::List => {
                let body =
                    render_repo_list(&self.state.repos, self.state.active_repo_id.as_deref());
                self.telegram.send_message(chat_id, &body, None).await?;
            }
            RepoCommand::Status => {
                let body = match self.state.active_repo() {
                    Some(repo) => render_repo_status(repo),
                    None => "No active repo. Use /repo list or /repo clone.".to_string(),
                };
                self.telegram.send_message(chat_id, &body, None).await?;
            }
            RepoCommand::Rescan => {
                let discovered = discover_workspace_repos(&self.config.workspace.root)?;
                let changed = merge_discovered_repos(&mut self.state, discovered);
                self.persist_state()?;
                let message = if changed {
                    "Workspace rescan completed and repo catalog updated."
                } else {
                    "Workspace rescan completed. No repo changes detected."
                };
                self.telegram.send_message(chat_id, message, None).await?;
            }
            RepoCommand::Use { repo } => {
                if self.state.active_turn_id.is_some() {
                    self.telegram
                        .send_message(
                            chat_id,
                            "A turn is in progress. Use /abort before switching repos.",
                            None,
                        )
                        .await?;
                    return Ok(());
                }
                let repo_id = self
                    .state
                    .resolve_repo_ref(&repo)
                    .map(|repo| repo.repo_id.clone())
                    .with_context(|| format!("repo not found: {repo}"))?;
                self.use_repo(repo_id).await?;
                let body = render_repo_status(
                    self.state
                        .active_repo()
                        .context("active repo must exist after /repo use")?,
                );
                self.telegram.send_message(chat_id, &body, None).await?;
            }
            RepoCommand::Clone { git_url, dir_name } => {
                if self.state.active_turn_id.is_some() {
                    self.telegram
                        .send_message(
                            chat_id,
                            "A turn is in progress. Finish it before cloning another repo.",
                            None,
                        )
                        .await?;
                    return Ok(());
                }
                let repo = clone_repo(&self.config, &git_url, dir_name.as_deref(), &mut self.state)
                    .await?;
                self.persist_state()?;
                self.telegram
                    .send_message(
                        chat_id,
                        &format!("Cloned repo {} into {}", repo.name, repo.path.display()),
                        None,
                    )
                    .await?;
            }
        }
        Ok(())
    }

    async fn handle_thread_command(&mut self, chat_id: i64, command: ThreadCommand) -> Result<()> {
        let active_repo_id = match self.state.active_repo_id.clone() {
            Some(repo_id) => repo_id,
            None => {
                self.telegram
                    .send_message(
                        chat_id,
                        "No active repo. Use /repo use or /repo clone first.",
                        None,
                    )
                    .await?;
                return Ok(());
            }
        };

        match command {
            ThreadCommand::List => {
                let body = render_thread_list(
                    self.state
                        .find_repo_by_id(&active_repo_id)
                        .context("active repo missing")?,
                );
                self.telegram.send_message(chat_id, &body, None).await?;
            }
            ThreadCommand::Status => {
                let repo = self
                    .state
                    .find_repo_by_id(&active_repo_id)
                    .context("active repo missing")?;
                let body = render_thread_status(repo, self.state.active_thread());
                self.telegram.send_message(chat_id, &body, None).await?;
            }
            ThreadCommand::New => {
                if self.state.active_turn_id.is_some() {
                    self.telegram
                        .send_message(
                            chat_id,
                            "A turn is in progress. Use /abort before opening a new thread.",
                            None,
                        )
                        .await?;
                    return Ok(());
                }
                let model = self.config.codex.model.clone();
                let runtime = self.ensure_runtime_for_repo(&active_repo_id).await?;
                let response = runtime.create_thread(model).await?;
                let title = format!("Thread {}", Utc::now().format("%Y-%m-%d %H:%M"));
                let thread = self.state.create_thread_for_repo(
                    &active_repo_id,
                    response.thread.id,
                    response.thread.path,
                    title,
                    false,
                )?;
                self.persist_state()?;
                self.telegram
                    .send_message(
                        chat_id,
                        &format!(
                            "Created new thread {} [{}]",
                            thread.title,
                            short_id(&thread.local_thread_id)
                        ),
                        None,
                    )
                    .await?;
            }
            ThreadCommand::Use { thread } => {
                if self.state.active_turn_id.is_some() {
                    self.telegram
                        .send_message(
                            chat_id,
                            "A turn is in progress. Use /abort before switching threads.",
                            None,
                        )
                        .await?;
                    return Ok(());
                }
                let repo = self
                    .state
                    .find_repo_by_id(&active_repo_id)
                    .context("active repo missing")?
                    .clone();
                let thread = self
                    .state
                    .resolve_thread_ref(&repo, &thread)
                    .cloned()
                    .context("thread not found")?;
                self.use_repo(active_repo_id.clone()).await?;
                let model = self.config.codex.model.clone();
                let runtime = self.ensure_runtime_for_repo(&active_repo_id).await?;
                let response = runtime
                    .resume_thread(
                        thread.codex_thread_id.clone(),
                        thread_resume_path(&thread),
                        model,
                    )
                    .await?;
                self.state.update_thread_runtime_metadata(
                    &active_repo_id,
                    &thread.local_thread_id,
                    response.thread.id,
                    response.thread.path,
                )?;
                self.state
                    .activate_thread(&active_repo_id, &thread.local_thread_id)?;
                self.persist_state()?;
                self.telegram
                    .send_message(
                        chat_id,
                        &format!(
                            "Switched to thread {} [{}]",
                            thread.title,
                            short_id(&thread.local_thread_id)
                        ),
                        None,
                    )
                    .await?;
            }
        }
        Ok(())
    }

    async fn handle_text(&mut self, chat_id: i64, text: String) -> Result<()> {
        if self.state.active_turn_id.is_some() {
            self.telegram
                .send_message(
                    chat_id,
                    "A turn is already running. Wait for it or use /abort.",
                    None,
                )
                .await?;
            return Ok(());
        }

        let repo_id = match self.state.active_repo_id.clone() {
            Some(repo_id) => repo_id,
            None => {
                self.telegram
                    .send_message(
                        chat_id,
                        "No active repo. Use /repo list, /repo use, or /repo clone first.",
                        None,
                    )
                    .await?;
                return Ok(());
            }
        };

        self.use_repo(repo_id.clone()).await?;

        let _repo = self
            .state
            .find_repo_by_id(&repo_id)
            .cloned()
            .context("active repo missing")?;
        let active_thread = if let Some(thread) = self.state.active_thread().cloned() {
            thread
        } else {
            let model = self.config.codex.model.clone();
            let response = self
                .ensure_runtime_for_repo(&repo_id)
                .await?
                .create_thread(model)
                .await?;
            let title = title_from_text(&text);
            let thread = self.state.create_thread_for_repo(
                &repo_id,
                response.thread.id,
                response.thread.path,
                title,
                true,
            )?;
            self.persist_state()?;
            thread
        };

        if !active_thread.has_user_message {
            let title = title_from_text(&text);
            self.state.update_active_thread_title(title);
            self.persist_state()?;
        }

        let model = self.config.codex.model.clone();
        let runtime = self.ensure_runtime_for_repo(&repo_id).await?;
        let response = runtime
            .start_turn(active_thread.codex_thread_id.clone(), text, model)
            .await?;

        let progress = self
            .telegram
            .send_message(chat_id, "Starting Codex turn...", None)
            .await?;

        self.state.active_turn_id = Some(response.turn.id.clone());
        self.state.progress_message_id = Some(progress.message_id);
        self.active_file_changes.clear();
        self.progress = Some(ActiveProgress {
            chat_id,
            repo_id: repo_id.clone(),
            thread_local_id: active_thread.local_thread_id.clone(),
            message_id: progress.message_id,
            assistant_text: String::new(),
            command_output_tail: String::new(),
            diff_preview: String::new(),
            last_rendered_at: None,
        });
        self.persist_state()?;
        self.render_progress(false).await?;
        Ok(())
    }

    async fn handle_callback(&mut self, callback: CallbackQuery) -> Result<()> {
        let data = callback.data.clone().unwrap_or_default();
        let parsed = parse_callback_action(&data);
        if parsed.is_none() {
            if data.starts_with("cmd:") || data.starts_with("patch:") || data.is_empty() {
                self.mark_callback_inactive(&callback, "no longer active")
                    .await?;
            } else {
                self.telegram
                    .answer_callback_query(&callback.id, Some("Unsupported action."))
                    .await?;
            }
            return Ok(());
        }
        let (action, callback_request_id) = parsed.expect("checked is_some above");

        let pending = match self.state.pending_request.clone() {
            Some(pending) => pending,
            None => {
                self.mark_callback_inactive(&callback, "no longer active")
                    .await?;
                return Ok(());
            }
        };
        if pending_request_id(&pending) != &callback_request_id {
            self.mark_callback_inactive(&callback, "no longer active")
                .await?;
            return Ok(());
        }

        match (pending, action.as_str()) {
            (
                PendingRequest::CommandApproval {
                    request_id,
                    repo_id,
                    thread_title: _,
                    ..
                },
                "cmd:approve",
            ) => {
                self.use_repo(repo_id.clone()).await?;
                self.ensure_runtime_for_repo(&repo_id)
                    .await?
                    .respond_command_approval(request_id, CommandExecutionApprovalDecision::Accept)
                    .await?;
                self.telegram
                    .answer_callback_query(&callback.id, Some("Command approved."))
                    .await?;
                self.cleanup_pending_request(Some("approved")).await?;
            }
            (
                PendingRequest::CommandApproval {
                    request_id,
                    repo_id,
                    command,
                    ..
                },
                "cmd:allow",
            ) => {
                let Some(command) = command else {
                    self.telegram
                        .answer_callback_query(
                            &callback.id,
                            Some("Only concrete commands can be saved."),
                        )
                        .await?;
                    return Ok(());
                };
                let rule = self.state.add_approval_rule(&repo_id, command.clone());
                self.persist_state()?;
                self.use_repo(repo_id.clone()).await?;
                self.ensure_runtime_for_repo(&repo_id)
                    .await?
                    .respond_command_approval(request_id, CommandExecutionApprovalDecision::Accept)
                    .await?;
                self.telegram
                    .answer_callback_query(&callback.id, Some("Command approved and saved."))
                    .await?;
                self.cleanup_pending_request(Some(&format!(
                    "approved and saved [{}]",
                    short_id(&rule.rule_id)
                )))
                .await?;
            }
            (
                PendingRequest::CommandApproval {
                    request_id,
                    repo_id,
                    ..
                },
                "cmd:decline",
            ) => {
                self.use_repo(repo_id.clone()).await?;
                self.ensure_runtime_for_repo(&repo_id)
                    .await?
                    .respond_command_approval(request_id, CommandExecutionApprovalDecision::Decline)
                    .await?;
                self.telegram
                    .answer_callback_query(&callback.id, Some("Command declined."))
                    .await?;
                self.cleanup_pending_request(Some("declined")).await?;
            }
            (
                PendingRequest::CommandApproval {
                    request_id,
                    repo_id,
                    ..
                },
                "cmd:abort",
            ) => {
                self.use_repo(repo_id.clone()).await?;
                self.ensure_runtime_for_repo(&repo_id)
                    .await?
                    .respond_command_approval(request_id, CommandExecutionApprovalDecision::Cancel)
                    .await?;
                self.telegram
                    .answer_callback_query(&callback.id, Some("Turn abort requested."))
                    .await?;
                self.cleanup_pending_request(Some("abort requested"))
                    .await?;
            }
            (
                PendingRequest::FileApproval {
                    request_id,
                    repo_id,
                    ..
                },
                "patch:approve",
            ) => {
                self.use_repo(repo_id.clone()).await?;
                self.ensure_runtime_for_repo(&repo_id)
                    .await?
                    .respond_file_approval(request_id, FileChangeApprovalDecision::Accept)
                    .await?;
                self.telegram
                    .answer_callback_query(&callback.id, Some("Patch approved."))
                    .await?;
                self.cleanup_pending_request(Some("approved")).await?;
            }
            (
                PendingRequest::FileApproval {
                    request_id,
                    repo_id,
                    ..
                },
                "patch:decline",
            ) => {
                self.use_repo(repo_id.clone()).await?;
                self.ensure_runtime_for_repo(&repo_id)
                    .await?
                    .respond_file_approval(request_id, FileChangeApprovalDecision::Decline)
                    .await?;
                self.telegram
                    .answer_callback_query(&callback.id, Some("Patch declined."))
                    .await?;
                self.cleanup_pending_request(Some("declined")).await?;
            }
            _ => {
                self.telegram
                    .answer_callback_query(&callback.id, Some("Unsupported action."))
                    .await?;
            }
        }
        Ok(())
    }

    async fn handle_codex_event(&mut self, event: CodexEvent) -> Result<()> {
        match event {
            CodexEvent::TurnStarted { repo_id, payload } => {
                if self.state.active_repo_id.as_deref() == Some(repo_id.as_str()) {
                    self.state.active_turn_id = Some(payload.turn.id);
                    self.persist_state()?;
                }
            }
            CodexEvent::TurnCompleted { repo_id, payload } => {
                if self.state.active_repo_id.as_deref() != Some(repo_id.as_str()) {
                    return Ok(());
                }
                self.finish_turn(
                    &payload.turn.status,
                    payload.turn.error.as_ref().map(|e| e.message.as_str()),
                )
                .await?;
            }
            CodexEvent::AgentMessageDelta { repo_id, payload } => {
                if let Some(progress) = self.progress.as_mut() {
                    if progress.repo_id == repo_id {
                        progress.assistant_text.push_str(&payload.delta);
                        self.render_progress(false).await?;
                    }
                }
            }
            CodexEvent::CommandOutputDelta { repo_id, payload } => {
                if let Some(progress) = self.progress.as_mut() {
                    if progress.repo_id == repo_id {
                        progress.command_output_tail.push_str(&payload.delta);
                        progress.command_output_tail =
                            trim_output_tail(&progress.command_output_tail, 4_000);
                        self.render_progress(false).await?;
                    }
                }
            }
            CodexEvent::DiffUpdated { repo_id, payload } => {
                if let Some(progress) = self.progress.as_mut() {
                    if progress.repo_id == repo_id {
                        progress.diff_preview = payload.diff;
                        self.render_progress(false).await?;
                    }
                }
            }
            CodexEvent::ItemStarted { repo_id, payload } => {
                if self.state.active_repo_id.as_deref() != Some(repo_id.as_str()) {
                    return Ok(());
                }
                if let ThreadItem::FileChange { id, changes, .. } = payload.item {
                    self.active_file_changes.insert(id, changes);
                }
            }
            CodexEvent::ItemCompleted { repo_id, payload } => {
                if self.state.active_repo_id.as_deref() != Some(repo_id.as_str()) {
                    return Ok(());
                }
                if let ThreadItem::FileChange { id, .. } = payload.item {
                    self.active_file_changes.remove(&id);
                }
            }
            CodexEvent::CommandApprovalRequested {
                repo_id,
                request_id,
                params,
            } => {
                self.on_command_approval(repo_id, request_id, params)
                    .await?;
            }
            CodexEvent::FileApprovalRequested {
                repo_id,
                request_id,
                params,
            } => {
                self.on_file_approval(repo_id, request_id, params).await?;
            }
            CodexEvent::ServerRequestResolved { repo_id, payload } => {
                if self.state.active_repo_id.as_deref() != Some(repo_id.as_str()) {
                    return Ok(());
                }
                if self
                    .state
                    .pending_request
                    .as_ref()
                    .map(|pending| pending_request_id(pending) == &payload.request_id)
                    .unwrap_or(false)
                {
                    self.cleanup_pending_request(Some("processed")).await?;
                }
            }
            CodexEvent::Error { repo_id, payload } => {
                if self.state.active_repo_id.as_deref() != Some(repo_id.as_str()) {
                    return Ok(());
                }
                warn!(
                    "codex turn error thread={} turn={} retry={}: {}",
                    payload.thread_id, payload.turn_id, payload.will_retry, payload.error.message
                );
            }
            CodexEvent::RuntimeExited {
                repo_id,
                status_code,
                error,
            } => {
                if self.state.active_runtime_repo_id.as_deref() == Some(repo_id.as_str()) {
                    self.state.active_runtime_repo_id = None;
                    self.persist_state()?;
                }
                warn!(
                    "codex runtime exited for repo {} status={:?} error={:?}",
                    repo_id, status_code, error
                );
            }
        }
        Ok(())
    }

    async fn on_command_approval(
        &mut self,
        repo_id: String,
        request_id: RpcId,
        params: crate::codex::protocol::CommandExecutionRequestApprovalParams,
    ) -> Result<()> {
        let repo = self
            .state
            .find_repo_by_id(&repo_id)
            .cloned()
            .context("approval repo missing")?;
        let thread = self
            .state
            .active_thread()
            .cloned()
            .context("approval thread missing")?;
        let chat_id = self.active_chat_id()?;
        let body = render_command_approval(
            &repo.name,
            &thread.title,
            params.cwd.as_ref().and_then(|path| path.to_str()),
            params.command.as_deref(),
            params.reason.as_deref(),
        );
        if let Some(command) = params.command.as_deref() {
            if let Some(rule) = self.state.find_matching_approval_rule(&repo_id, command) {
                let rule_id = rule.rule_id.clone();
                self.ensure_runtime_for_repo(&repo_id)
                    .await?
                    .respond_command_approval(request_id, CommandExecutionApprovalDecision::Accept)
                    .await?;
                info!(
                    "auto-approved command in repo {} via approval rule {}: {}",
                    repo.name,
                    short_id(&rule_id),
                    command
                );
                return Ok(());
            }
        }
        if self.state.pending_request.is_some() {
            self.cleanup_pending_request(Some("superseded by a newer approval"))
                .await?;
        }
        let approval_message = self
            .telegram
            .send_message(
                chat_id,
                &body,
                Some(&command_approval_keyboard(&request_id)),
            )
            .await?;
        self.state.pending_request = Some(PendingRequest::CommandApproval {
            request_id,
            repo_id,
            thread_local_id: thread.local_thread_id,
            thread_title: thread.title,
            approval_chat_id: chat_id,
            approval_message_id: approval_message.message_id,
            approval_message_text: body,
            turn_id: params.turn_id,
            item_id: params.item_id,
            command: params.command,
            cwd: params.cwd,
            reason: params.reason,
        });
        self.persist_state()?;
        Ok(())
    }

    async fn on_file_approval(
        &mut self,
        repo_id: String,
        request_id: RpcId,
        params: crate::codex::protocol::FileChangeRequestApprovalParams,
    ) -> Result<()> {
        let repo = self
            .state
            .find_repo_by_id(&repo_id)
            .cloned()
            .context("approval repo missing")?;
        let thread = self
            .state
            .active_thread()
            .cloned()
            .context("approval thread missing")?;
        let chat_id = self.active_chat_id()?;
        let changes = self
            .active_file_changes
            .get(&params.item_id)
            .cloned()
            .unwrap_or_default();
        let paths = changes
            .iter()
            .map(|change| change.path.clone())
            .collect::<Vec<_>>();
        let diff_preview = changes
            .iter()
            .map(|change| change.diff.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let patch_path = if diff_preview.len() > self.config.ui.max_inline_diff_chars {
            let path = self
                .config
                .temp_dir()
                .join(format!("{}.patch", params.item_id.replace('/', "_")));
            tokio::fs::write(&path, diff_preview.as_bytes())
                .await
                .with_context(|| format!("failed to write {}", path.display()))?;
            Some(path)
        } else {
            None
        };

        let caption = render_file_approval(
            &repo.name,
            &thread.title,
            &paths,
            params.reason.as_deref(),
            &diff_preview,
        );
        if self.state.pending_request.is_some() {
            self.cleanup_pending_request(Some("superseded by a newer approval"))
                .await?;
        }

        let pending_request = if let Some(path) = patch_path {
            self.telegram
                .send_document(chat_id, &path, &caption)
                .await?;
            let message = self
                .telegram
                .send_message(
                    chat_id,
                    "Use the buttons below to approve or decline this patch.",
                    Some(&file_approval_keyboard(&request_id)),
                )
                .await?;
            PendingRequest::FileApproval {
                request_id,
                repo_id: repo_id.clone(),
                thread_local_id: thread.local_thread_id.clone(),
                thread_title: thread.title.clone(),
                approval_chat_id: chat_id,
                approval_message_id: message.message_id,
                approval_message_text: "Use the buttons below to approve or decline this patch."
                    .into(),
                turn_id: params.turn_id.clone(),
                item_id: params.item_id.clone(),
                paths: paths.clone(),
                reason: params.reason.clone(),
                diff_preview: diff_preview.clone(),
                patch_path: Some(path),
                preferred_decision: FileChangeApprovalDecision::Accept,
            }
        } else {
            let message = self
                .telegram
                .send_message(
                    chat_id,
                    &caption,
                    Some(&file_approval_keyboard(&request_id)),
                )
                .await?;
            PendingRequest::FileApproval {
                request_id,
                repo_id: repo_id.clone(),
                thread_local_id: thread.local_thread_id.clone(),
                thread_title: thread.title.clone(),
                approval_chat_id: chat_id,
                approval_message_id: message.message_id,
                approval_message_text: caption,
                turn_id: params.turn_id.clone(),
                item_id: params.item_id.clone(),
                paths: paths.clone(),
                reason: params.reason.clone(),
                diff_preview: diff_preview.clone(),
                patch_path: None,
                preferred_decision: FileChangeApprovalDecision::Accept,
            }
        };
        self.state.pending_request = Some(pending_request);
        self.persist_state()?;
        Ok(())
    }

    async fn finish_turn(
        &mut self,
        status: &TurnStatus,
        error_message: Option<&str>,
    ) -> Result<()> {
        let final_text = if let Some(progress) = self.progress.as_ref() {
            if progress.assistant_text.is_empty() {
                status_label(status).to_string()
            } else {
                progress.assistant_text.clone()
            }
        } else {
            status_label(status).to_string()
        };

        self.render_progress(true).await?;

        if let Some(progress) = &self.progress {
            let chunks = split_message(&final_text);
            if chunks.len() > 1 {
                let summary = format!(
                    "repo: {}\nthread: {}\nstatus: {}\n\nFull response sent in {} parts.",
                    self.state
                        .find_repo_by_id(&progress.repo_id)
                        .map(|repo| repo.name.clone())
                        .unwrap_or_else(|| "unknown".into()),
                    self.state
                        .find_repo_by_id(&progress.repo_id)
                        .and_then(|repo| repo
                            .threads
                            .iter()
                            .find(|thread| thread.local_thread_id == progress.thread_local_id))
                        .map(|thread| thread.title.clone())
                        .unwrap_or_else(|| "unknown".into()),
                    status_label(status),
                    chunks.len()
                );
                let _ = self
                    .telegram
                    .edit_message_text(progress.chat_id, progress.message_id, &summary, None)
                    .await;
                for chunk in chunks {
                    self.telegram
                        .send_message(progress.chat_id, &chunk, None)
                        .await?;
                }
            }
        }

        if let Some(error_message) = error_message {
            if let Some(chat_id) = self.progress.as_ref().map(|progress| progress.chat_id) {
                self.telegram
                    .send_message(
                        chat_id,
                        &format!("Turn ended with error: {error_message}"),
                        None,
                    )
                    .await?;
            }
        }

        self.state.active_turn_id = None;
        self.state.progress_message_id = None;
        self.cleanup_pending_request(Some("no longer active"))
            .await?;
        self.progress = None;
        self.active_file_changes.clear();
        self.persist_state()?;
        Ok(())
    }

    async fn abort_active_turn(&mut self, chat_id: i64) -> Result<()> {
        let turn_id = match self.state.active_turn_id.clone() {
            Some(turn_id) => turn_id,
            None => {
                self.telegram
                    .send_message(chat_id, "No active turn is running.", None)
                    .await?;
                return Ok(());
            }
        };
        let thread = self
            .state
            .active_thread()
            .cloned()
            .context("active thread missing")?;
        let repo_id = self
            .state
            .active_repo_id
            .clone()
            .context("active repo missing")?;
        self.ensure_runtime_for_repo(&repo_id)
            .await?
            .interrupt_turn(thread.codex_thread_id, turn_id)
            .await?;
        self.telegram
            .send_message(chat_id, "Turn interrupt requested.", None)
            .await?;
        Ok(())
    }

    async fn render_progress(&mut self, force: bool) -> Result<()> {
        let Some(progress) = self.progress.as_mut() else {
            return Ok(());
        };

        let should_render = force
            || progress
                .last_rendered_at
                .map(|last| {
                    last.elapsed() >= Duration::from_millis(self.config.ui.stream_edit_interval_ms)
                })
                .unwrap_or(true);
        if !should_render {
            return Ok(());
        }

        let repo = self
            .state
            .find_repo_by_id(&progress.repo_id)
            .context("progress repo missing")?;
        let thread = repo
            .threads
            .iter()
            .find(|thread| thread.local_thread_id == progress.thread_local_id)
            .context("progress thread missing")?;
        let view = ProgressView {
            repo_name: repo.name.clone(),
            thread_title: thread.title.clone(),
            status: status_label_from_state(self.state.active_turn_id.as_deref(), force),
            assistant_text: progress.assistant_text.clone(),
            command_output_tail: progress.command_output_tail.clone(),
            diff_preview: progress.diff_preview.clone(),
        };

        let rendered = render_progress(&view);
        self.telegram
            .edit_message_text(progress.chat_id, progress.message_id, &rendered, None)
            .await?;
        progress.last_rendered_at = Some(Instant::now());
        Ok(())
    }

    async fn cleanup_pending_request(&mut self, status: Option<&str>) -> Result<()> {
        if let Some(pending) = self.state.pending_request.clone() {
            if let Some(status) = status {
                if let Err(err) = self.update_pending_request_message(&pending, status).await {
                    warn!("failed to update pending approval message: {err}");
                }
            }
            if let PendingRequest::FileApproval {
                patch_path: Some(path),
                ..
            } = &pending
            {
                let _ = tokio::fs::remove_file(path).await;
            }
        }
        self.state.pending_request = None;
        self.persist_state()?;
        Ok(())
    }

    async fn reconcile_runtime_exit(&mut self, chat_id: Option<i64>) -> Result<()> {
        let exit_status = match self.runtime.as_mut() {
            Some(runtime) => runtime.try_wait()?,
            None => None,
        };
        let Some(exit_status) = exit_status else {
            return Ok(());
        };

        let runtime = self
            .runtime
            .take()
            .context("runtime must exist after try_wait")?;
        let repo_id = runtime.repo_id().to_string();
        let repo_name = self
            .state
            .find_repo_by_id(&repo_id)
            .map(|repo| repo.name.clone())
            .unwrap_or_else(|| repo_id.clone());
        let progress_snapshot = self.progress.clone();
        let had_pending_request = self.state.pending_request.is_some();
        let had_active_turn = self.state.active_turn_id.is_some() || progress_snapshot.is_some();
        let _ = runtime.stop().await;

        if self.state.active_runtime_repo_id.as_deref() == Some(repo_id.as_str()) {
            self.state.active_runtime_repo_id = None;
        }

        if let Some(progress) = progress_snapshot.as_ref() {
            let repo_label = self
                .state
                .find_repo_by_id(&progress.repo_id)
                .map(|repo| repo.name.clone())
                .unwrap_or_else(|| progress.repo_id.clone());
            let thread_label = self
                .state
                .find_repo_by_id(&progress.repo_id)
                .and_then(|repo| {
                    repo.threads
                        .iter()
                        .find(|thread| thread.local_thread_id == progress.thread_local_id)
                })
                .map(|thread| thread.title.clone())
                .unwrap_or_else(|| progress.thread_local_id.clone());
            let _ = self
                .telegram
                .edit_message_text(
                    progress.chat_id,
                    progress.message_id,
                    &format!("repo: {repo_label}\nthread: {thread_label}\nstatus: runtime exited"),
                    None,
                )
                .await;
        }

        if had_pending_request {
            self.cleanup_pending_request(Some("no longer active"))
                .await?;
        }

        self.state.active_turn_id = None;
        self.state.progress_message_id = None;
        self.progress = None;
        self.active_file_changes.clear();
        self.persist_state()?;

        warn!(
            "codex runtime exited for repo {} status={:?}; cleared active turn state",
            repo_id,
            exit_status.code()
        );

        if had_active_turn || had_pending_request {
            let notify_chat_id = chat_id
                .or_else(|| progress_snapshot.as_ref().map(|progress| progress.chat_id))
                .or(self.config.telegram.allowed_chat_id);
            if let Some(chat_id) = notify_chat_id {
                let status = exit_status
                    .code()
                    .map(|code| format!("status {code}"))
                    .unwrap_or_else(|| "an unknown status".to_string());
                self.telegram
                    .send_message(
                        chat_id,
                        &format!(
                            "Codex runtime exited unexpectedly for repo {repo_name} ({status}). Cleared the stuck turn; send your message again."
                        ),
                        None,
                    )
                    .await?;
            }
        }

        Ok(())
    }

    async fn update_pending_request_message(
        &self,
        pending: &PendingRequest,
        status: &str,
    ) -> Result<()> {
        let Some((chat_id, message_id, text)) = pending_request_message_meta(pending) else {
            return Ok(());
        };
        let updated = approval_message_with_status(text, status);
        self.telegram
            .edit_message_text(chat_id, message_id, &updated, None)
            .await?;
        Ok(())
    }

    async fn mark_callback_inactive(&self, callback: &CallbackQuery, status: &str) -> Result<()> {
        self.telegram
            .answer_callback_query(&callback.id, Some("This approval is no longer active."))
            .await?;
        let Some(message) = callback.message.as_ref() else {
            return Ok(());
        };
        let Some(text) = message.text.as_deref() else {
            return Ok(());
        };
        let updated = approval_message_with_status(text, status);
        let _ = self
            .telegram
            .edit_message_text(message.chat.id, message.message_id, &updated, None)
            .await;
        Ok(())
    }

    async fn use_repo(&mut self, repo_id: String) -> Result<()> {
        if self.state.active_repo_id.as_deref() == Some(repo_id.as_str()) {
            self.ensure_runtime_for_repo(&repo_id).await?;
            return Ok(());
        }

        if let Some(runtime) = self.runtime.take() {
            runtime.stop().await?;
        }

        self.state.set_active_repo(repo_id.clone());
        self.state.active_runtime_repo_id = None;
        self.state.mark_repo_used(&repo_id);
        self.persist_state()?;
        self.ensure_runtime_for_repo(&repo_id).await?;
        Ok(())
    }

    async fn ensure_runtime_for_repo(&mut self, repo_id: &str) -> Result<&mut CodexRuntime> {
        if self.state.active_runtime_repo_id.as_deref() == Some(repo_id) && self.runtime.is_some() {
            return self
                .runtime
                .as_mut()
                .context("runtime marked active but missing");
        }

        if let Some(runtime) = self.runtime.take() {
            runtime.stop().await?;
        }

        let repo = self
            .state
            .find_repo_by_id(repo_id)
            .cloned()
            .with_context(|| format!("repo not found: {repo_id}"))?;
        let active_thread = repo.active_thread().cloned();
        let mut runtime = CodexRuntime::start(
            &self.config.codex.bin,
            repo.repo_id.clone(),
            repo.path.clone(),
            self.codex_events_tx.clone(),
        )
        .await?;
        if let Some(thread) = active_thread {
            let model = self.config.codex.model.clone();
            let response = match runtime
                .resume_thread(
                    thread.codex_thread_id.clone(),
                    thread_resume_path(&thread),
                    model,
                )
                .await
            {
                Ok(response) => response,
                Err(err) => {
                    if is_missing_thread_resume_error(&err) {
                        let cleared = self.state.clear_active_thread(repo_id)?;
                        warn!(
                            "failed to resume active thread {} [{}] for repo {}: {}; cleared active thread and continuing with a fresh runtime",
                            cleared
                                .as_ref()
                                .map(|thread| thread.title.as_str())
                                .unwrap_or(thread.title.as_str()),
                            short_id(&thread.local_thread_id),
                            repo.name,
                            err
                        );
                        self.state.active_runtime_repo_id = Some(repo.repo_id.clone());
                        self.persist_state()?;
                        self.runtime = Some(runtime);
                        return self
                            .runtime
                            .as_mut()
                            .context("runtime must exist after startup");
                    }
                    let _ = runtime.stop().await;
                    return Err(err).with_context(|| {
                        format!(
                            "failed to resume active thread {} [{}] for repo {}",
                            thread.title,
                            short_id(&thread.local_thread_id),
                            repo.name
                        )
                    });
                }
            };
            self.state.update_thread_runtime_metadata(
                repo_id,
                &thread.local_thread_id,
                response.thread.id,
                response.thread.path,
            )?;
        }
        self.state.active_runtime_repo_id = Some(repo.repo_id.clone());
        self.persist_state()?;
        self.runtime = Some(runtime);
        self.runtime
            .as_mut()
            .context("runtime must exist after startup")
    }

    fn persist_state(&self) -> Result<()> {
        self.state_store.save(&self.state)
    }

    fn reload_pairing_state(&mut self) -> Result<()> {
        let disk_state = self.state_store.load()?;
        self.state.approved_telegram_peers = disk_state.approved_telegram_peers;
        self.state.pending_pairings = disk_state.pending_pairings;
        Ok(())
    }

    fn active_chat_id(&self) -> Result<i64> {
        if let Some(progress) = &self.progress {
            return Ok(progress.chat_id);
        }
        self.config
            .telegram
            .allowed_chat_id
            .context("no active chat id available")
    }

    async fn handle_pairing_message(&mut self, message: &TelegramMessage) -> Result<()> {
        let from = message
            .from
            .as_ref()
            .context("pairing message is missing sender information")?;
        let request = self.state.ensure_pairing_request(
            from.id,
            message.chat.id,
            from.first_name.clone(),
            from.username.clone(),
        );
        self.persist_state()?;

        let body = format!(
            "Pairing required.\n\nCode: {}\n\nApprove it on the server with:\nmycodex pairing approve {}",
            request.code, request.code
        );
        self.telegram
            .send_message(message.chat.id, &body, None)
            .await?;
        Ok(())
    }
}

async fn probe_codex(config: &Config) -> Result<()> {
    let (events_tx, _events_rx) = mpsc::channel(1);
    let runtime = CodexRuntime::start(
        &config.codex.bin,
        "probe".to_string(),
        config.workspace.root.clone(),
        events_tx,
    )
    .await?;
    runtime.stop().await?;
    Ok(())
}

fn command_approval_keyboard(request_id: &RpcId) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup {
        inline_keyboard: vec![
            vec![
                InlineKeyboardButton {
                    text: "Approve once".into(),
                    callback_data: encode_callback_action("cmd:approve", request_id),
                },
                InlineKeyboardButton {
                    text: "Always allow".into(),
                    callback_data: encode_callback_action("cmd:allow", request_id),
                },
            ],
            vec![
                InlineKeyboardButton {
                    text: "Decline".into(),
                    callback_data: encode_callback_action("cmd:decline", request_id),
                },
                InlineKeyboardButton {
                    text: "Abort turn".into(),
                    callback_data: encode_callback_action("cmd:abort", request_id),
                },
            ],
        ],
    }
}

fn file_approval_keyboard(request_id: &RpcId) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup {
        inline_keyboard: vec![vec![
            InlineKeyboardButton {
                text: "Approve patch".into(),
                callback_data: encode_callback_action("patch:approve", request_id),
            },
            InlineKeyboardButton {
                text: "Decline patch".into(),
                callback_data: encode_callback_action("patch:decline", request_id),
            },
        ]],
    }
}

fn trim_output_tail(value: &str, max_len: usize) -> String {
    if value.len() <= max_len {
        return value.to_string();
    }
    value[value.len() - max_len..].to_string()
}

fn status_label(status: &TurnStatus) -> &'static str {
    match status {
        TurnStatus::Completed => "completed",
        TurnStatus::Interrupted => "interrupted",
        TurnStatus::Failed => "failed",
        TurnStatus::InProgress => "in progress",
    }
}

fn status_label_from_state(active_turn_id: Option<&str>, force: bool) -> String {
    if active_turn_id.is_some() && !force {
        "running".to_string()
    } else {
        "finished".to_string()
    }
}

fn pending_request_id(value: &PendingRequest) -> &RpcId {
    match value {
        PendingRequest::CommandApproval { request_id, .. } => request_id,
        PendingRequest::FileApproval { request_id, .. } => request_id,
    }
}

fn pending_request_message_meta(value: &PendingRequest) -> Option<(i64, i64, &str)> {
    match value {
        PendingRequest::CommandApproval {
            approval_chat_id,
            approval_message_id,
            approval_message_text,
            ..
        }
        | PendingRequest::FileApproval {
            approval_chat_id,
            approval_message_id,
            approval_message_text,
            ..
        } => Some((
            *approval_chat_id,
            *approval_message_id,
            approval_message_text.as_str(),
        )),
    }
}

fn encode_callback_action(action: &str, request_id: &RpcId) -> String {
    format!("{action}:{}", encode_request_id_token(request_id))
}

fn parse_callback_action(data: &str) -> Option<(String, RpcId)> {
    let (action, encoded_request_id) = data.rsplit_once(':')?;
    let request_id = parse_request_id_token(encoded_request_id)?;
    Some((action.to_string(), request_id))
}

fn encode_request_id_token(request_id: &RpcId) -> String {
    match request_id {
        RpcId::Number(value) => format!("n{value}"),
        RpcId::String(value) => {
            let mut token = String::with_capacity(1 + value.len() * 2);
            token.push('s');
            for byte in value.as_bytes() {
                use std::fmt::Write as _;
                let _ = write!(&mut token, "{byte:02x}");
            }
            token
        }
    }
}

fn parse_request_id_token(encoded: &str) -> Option<RpcId> {
    let (kind, payload) = encoded.split_at(1);
    match kind {
        "n" => payload.parse().ok().map(RpcId::Number),
        "s" => {
            let bytes = decode_hex(payload)?;
            let text = String::from_utf8(bytes).ok()?;
            Some(RpcId::String(text))
        }
        _ => None,
    }
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if value.len() % 2 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    let mut chunks = value.as_bytes().chunks_exact(2);
    for chunk in &mut chunks {
        let text = std::str::from_utf8(chunk).ok()?;
        let byte = u8::from_str_radix(text, 16).ok()?;
        bytes.push(byte);
    }
    Some(bytes)
}

fn approval_message_with_status(text: &str, status: &str) -> String {
    let base = text.split("\n\nStatus:").next().unwrap_or(text);
    let suffix = format!("\n\nStatus: {status}");
    const MAX_LEN: usize = 4_000;
    let base_chars = base.chars().collect::<Vec<_>>();
    let suffix_len = suffix.chars().count();
    if base_chars.len() + suffix_len <= MAX_LEN {
        return format!("{base}{suffix}");
    }

    let truncated_marker = "\n\n[truncated]";
    let keep = MAX_LEN.saturating_sub(suffix_len + truncated_marker.chars().count());
    let truncated = base_chars.into_iter().take(keep).collect::<String>();
    format!("{truncated}{truncated_marker}{suffix}")
}

fn thread_resume_path(thread: &ThreadRecord) -> Option<PathBuf> {
    thread
        .codex_thread_path
        .clone()
        .or_else(|| discover_rollout_path(&thread.codex_thread_id))
}

fn discover_rollout_path(thread_id: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let sessions_dir = PathBuf::from(home).join(".codex").join("sessions");
    discover_rollout_path_in_dir(&sessions_dir, thread_id)
}

fn discover_rollout_path_in_dir(dir: &Path, thread_id: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = discover_rollout_path_in_dir(&path, thread_id) {
                return Some(found);
            }
            continue;
        }
        let file_name = path.file_name()?.to_str()?;
        if file_name.ends_with(".jsonl") && file_name.contains(thread_id) {
            return Some(path);
        }
    }
    None
}

fn is_missing_thread_resume_error(err: &anyhow::Error) -> bool {
    let message = err.to_string();
    [
        "thread not found",
        "no rollout found",
        "failed to load rollout",
        "No such file or directory",
    ]
    .iter()
    .any(|pattern| message.contains(pattern))
}

enum MessageAccess {
    Allowed,
    NeedsPairing,
    Denied,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ThreadStatusRecord;
    use tempfile::tempdir;

    #[test]
    fn thread_resume_path_prefers_stored_path() {
        let thread = ThreadRecord {
            local_thread_id: "local-1".into(),
            codex_thread_id: "codex-1".into(),
            codex_thread_path: Some(PathBuf::from("/tmp/thread.jsonl")),
            repo_id: "repo-1".into(),
            title: "demo".into(),
            status: ThreadStatusRecord::Active,
            created_at: Utc::now(),
            last_used_at: Utc::now(),
            has_user_message: true,
        };

        assert_eq!(
            thread_resume_path(&thread),
            Some(PathBuf::from("/tmp/thread.jsonl"))
        );
    }

    #[test]
    fn discover_rollout_path_in_dir_finds_matching_file() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("2026").join("03").join("08");
        fs::create_dir_all(&nested).unwrap();
        let path = nested.join("rollout-2026-03-08T00-00-00-thread-123.jsonl");
        fs::write(&path, b"{}").unwrap();

        let found = discover_rollout_path_in_dir(dir.path(), "thread-123");
        assert_eq!(found, Some(path));
    }

    #[test]
    fn callback_action_roundtrips_numeric_request_id() {
        let encoded = encode_callback_action("cmd:approve", &RpcId::Number(42));
        let parsed = parse_callback_action(&encoded);
        assert_eq!(parsed, Some(("cmd:approve".into(), RpcId::Number(42))));
    }

    #[test]
    fn callback_action_roundtrips_string_request_id() {
        let encoded = encode_callback_action("patch:decline", &RpcId::String("req:abc".into()));
        let parsed = parse_callback_action(&encoded);
        assert_eq!(
            parsed,
            Some(("patch:decline".into(), RpcId::String("req:abc".into())))
        );
    }

    #[test]
    fn approval_message_status_replaces_existing_status_line() {
        let rendered = approval_message_with_status("hello\n\nStatus: old", "approved");
        assert_eq!(rendered, "hello\n\nStatus: approved");
    }
}
