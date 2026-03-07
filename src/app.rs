use std::collections::HashMap;
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
use crate::commands::{Command, RepoCommand, ThreadCommand, UserInput, parse_user_input};
use crate::config::{Config, TelegramAccessMode};
use crate::repo::{clone_repo, discover_workspace_repos, merge_discovered_repos};
use crate::state::{AppState, PendingRequest, StateStore};
use crate::telegram::api::{
    CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, TelegramClient, TelegramMessage,
    Update,
};
use crate::telegram::render::{
    ProgressView, render_command_approval, render_file_approval, render_help, render_progress,
    render_repo_list, render_repo_status, render_status, render_thread_list, render_thread_status,
    short_id, split_message, title_from_text,
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
            Command::Status => {
                let body = render_status(
                    self.state.active_repo(),
                    self.state.active_thread(),
                    self.state.active_runtime_repo_id.as_deref(),
                    self.state.active_turn_id.as_deref(),
                    self.state.pending_request.as_ref(),
                );
                self.telegram.send_message(chat_id, &body, None).await?;
            }
            Command::Abort => {
                self.abort_active_turn(chat_id).await?;
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
                runtime
                    .resume_thread(thread.codex_thread_id.clone(), model)
                    .await?;
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
            let thread =
                self.state
                    .create_thread_for_repo(&repo_id, response.thread.id, title, true)?;
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
        let pending = match self.state.pending_request.clone() {
            Some(pending) => pending,
            None => {
                self.telegram
                    .answer_callback_query(&callback.id, Some("This approval is no longer active."))
                    .await?;
                return Ok(());
            }
        };

        match (pending, data.as_str()) {
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
                    self.cleanup_pending_request().await?;
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
        self.state.pending_request = Some(PendingRequest::CommandApproval {
            request_id,
            repo_id,
            thread_local_id: thread.local_thread_id,
            thread_title: thread.title,
            turn_id: params.turn_id,
            item_id: params.item_id,
            command: params.command,
            cwd: params.cwd,
            reason: params.reason,
        });
        self.persist_state()?;
        self.telegram
            .send_message(chat_id, &body, Some(&command_approval_keyboard()))
            .await?;
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

        self.state.pending_request = Some(PendingRequest::FileApproval {
            request_id,
            repo_id: repo_id.clone(),
            thread_local_id: thread.local_thread_id.clone(),
            thread_title: thread.title.clone(),
            turn_id: params.turn_id.clone(),
            item_id: params.item_id.clone(),
            paths: paths.clone(),
            reason: params.reason.clone(),
            diff_preview: diff_preview.clone(),
            patch_path: patch_path.clone(),
            preferred_decision: FileChangeApprovalDecision::Accept,
        });
        self.persist_state()?;

        let caption = render_file_approval(
            &repo.name,
            &thread.title,
            &paths,
            params.reason.as_deref(),
            &diff_preview,
        );

        if let Some(path) = patch_path {
            self.telegram
                .send_document(chat_id, &path, &caption)
                .await?;
            self.telegram
                .send_message(
                    chat_id,
                    "Use the buttons below to approve or decline this patch.",
                    Some(&file_approval_keyboard()),
                )
                .await?;
        } else {
            self.telegram
                .send_message(chat_id, &caption, Some(&file_approval_keyboard()))
                .await?;
        }
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
        self.state.pending_request = None;
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

    async fn cleanup_pending_request(&mut self) -> Result<()> {
        if let Some(PendingRequest::FileApproval {
            patch_path: Some(path),
            ..
        }) = self.state.pending_request.as_ref()
        {
            let _ = tokio::fs::remove_file(path).await;
        }
        self.state.pending_request = None;
        self.persist_state()?;
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
        let runtime = CodexRuntime::start(
            &self.config.codex.bin,
            repo.repo_id.clone(),
            repo.path.clone(),
            self.codex_events_tx.clone(),
        )
        .await?;
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

fn command_approval_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup {
        inline_keyboard: vec![vec![
            InlineKeyboardButton {
                text: "Approve once".into(),
                callback_data: "cmd:approve".into(),
            },
            InlineKeyboardButton {
                text: "Decline".into(),
                callback_data: "cmd:decline".into(),
            },
            InlineKeyboardButton {
                text: "Abort turn".into(),
                callback_data: "cmd:abort".into(),
            },
        ]],
    }
}

fn file_approval_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup {
        inline_keyboard: vec![vec![
            InlineKeyboardButton {
                text: "Approve patch".into(),
                callback_data: "patch:approve".into(),
            },
            InlineKeyboardButton {
                text: "Decline patch".into(),
                callback_data: "patch:decline".into(),
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

enum MessageAccess {
    Allowed,
    NeedsPairing,
    Denied,
}
