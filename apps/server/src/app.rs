use std::collections::HashMap;
use std::fs;
use std::future::pending;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::time::{interval, sleep};
use tracing::{info, warn};

use crate::app_auth::AppAuthStore;
use crate::app_gateway::{
    AppApprovalDecision, AppControlCommand, AppControlError, AppGatewayHandle,
};
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
use crate::state::{AppState, StateStore, ThreadRecord, ThreadSurface};
use crate::telegram::api::{
    BotCommand, CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, TelegramClient,
    TelegramMessage, Update, default_bot_commands,
};
use crate::telegram::render::{
    ProgressView, render_approval_menu, render_approval_remove_menu, render_approval_rules,
    render_command_approval, render_file_approval, render_help, render_progress,
    render_repo_clone_menu, render_repo_list, render_repo_menu, render_repo_status,
    render_repo_use_menu, render_status, render_thread_list, render_thread_menu,
    render_thread_status, render_thread_use_menu, short_id, split_message, title_from_text,
};

pub struct App {
    config: Config,
    telegram: TelegramBridge,
    state_store: StateStore,
    state: AppState,
    runtimes: HashMap<String, CodexRuntime>,
    codex_events_tx: mpsc::Sender<CodexEvent>,
    codex_events_rx: mpsc::Receiver<CodexEvent>,
    app_control_rx: mpsc::Receiver<AppControlCommand>,
    app_gateway: Option<AppGatewayHandle>,
    update_offset: i64,
    active_runs: HashMap<String, ActiveRun>,
    active_file_changes: HashMap<String, HashMap<String, Vec<FileUpdateChange>>>,
}

#[derive(Debug, Clone)]
struct ActiveRun {
    surface: ThreadSurface,
    thread_local_id: String,
    turn_id: String,
    assistant_text: String,
    command_output_tail: String,
    diff_preview: String,
    route: RunRoute,
    pending_request: Option<RuntimePendingRequest>,
}

#[derive(Debug, Clone)]
enum RunRoute {
    Telegram {
        chat_id: i64,
        message_id: i64,
        last_rendered_at: Option<Instant>,
    },
    App {
        device_id: String,
    },
}

#[derive(Debug, Clone)]
enum RuntimePendingRequest {
    Command(CommandPendingRequest),
    File(FilePendingRequest),
}

#[derive(Debug, Clone)]
struct CommandPendingRequest {
    request_id: RpcId,
    thread_title: String,
    turn_id: String,
    item_id: String,
    command: Option<String>,
    cwd: Option<PathBuf>,
    reason: Option<String>,
    telegram_message: Option<TelegramApprovalMessage>,
}

#[derive(Debug, Clone)]
struct FilePendingRequest {
    request_id: RpcId,
    thread_title: String,
    turn_id: String,
    item_id: String,
    paths: Vec<String>,
    reason: Option<String>,
    diff_preview: String,
    patch_path: Option<PathBuf>,
    preferred_decision: FileChangeApprovalDecision,
    telegram_message: Option<TelegramApprovalMessage>,
}

#[derive(Debug, Clone)]
struct TelegramApprovalMessage {
    chat_id: i64,
    message_id: i64,
    text: String,
}

#[derive(Debug, Clone)]
struct TelegramBridge {
    client: Option<TelegramClient>,
}

impl TelegramBridge {
    fn enabled(bot_token: &str) -> Self {
        Self {
            client: Some(TelegramClient::new(bot_token)),
        }
    }

    fn disabled() -> Self {
        Self { client: None }
    }

    async fn set_my_commands(&self, commands: &[BotCommand]) -> Result<()> {
        match &self.client {
            Some(client) => client.set_my_commands(commands).await,
            None => Ok(()),
        }
    }

    async fn get_me(&self) -> Result<crate::telegram::api::TelegramUser> {
        match &self.client {
            Some(client) => client.get_me().await,
            None => bail!("telegram is disabled"),
        }
    }

    async fn get_updates(&self, offset: i64, timeout: u64) -> Result<Vec<Update>> {
        match &self.client {
            Some(client) => client.get_updates(offset, timeout).await,
            None => pending::<Result<Vec<Update>>>().await,
        }
    }

    async fn send_message(
        &self,
        chat_id: i64,
        text: &str,
        keyboard: Option<&InlineKeyboardMarkup>,
    ) -> Result<TelegramMessage> {
        match &self.client {
            Some(client) => client.send_message(chat_id, text, keyboard).await,
            None => bail!("telegram is disabled"),
        }
    }

    async fn edit_message_text(
        &self,
        chat_id: i64,
        message_id: i64,
        text: &str,
        keyboard: Option<&InlineKeyboardMarkup>,
    ) -> Result<TelegramMessage> {
        match &self.client {
            Some(client) => {
                client
                    .edit_message_text(chat_id, message_id, text, keyboard)
                    .await
            }
            None => bail!("telegram is disabled"),
        }
    }

    async fn answer_callback_query(&self, query_id: &str, text: Option<&str>) -> Result<()> {
        match &self.client {
            Some(client) => client.answer_callback_query(query_id, text).await,
            None => bail!("telegram is disabled"),
        }
    }

    async fn send_document(
        &self,
        chat_id: i64,
        file_path: &Path,
        caption: &str,
    ) -> Result<TelegramMessage> {
        match &self.client {
            Some(client) => client.send_document(chat_id, file_path, caption).await,
            None => bail!("telegram is disabled"),
        }
    }
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
        let auth_store = AppAuthStore::new(config.app_auth_file());
        let mut state = state_store.load()?;
        state.clear_stale_runtime_state();
        let discovered = discover_workspace_repos(&config.workspace.root)?;
        let _changed = merge_discovered_repos(&mut state, discovered);
        state_store.save(&state)?;

        let telegram = if config.telegram.is_enabled() {
            let telegram = TelegramBridge::enabled(&config.telegram.bot_token);
            if let Err(err) = telegram.set_my_commands(&default_bot_commands()).await {
                warn!("failed to register Telegram bot commands: {err}");
            }
            telegram
        } else {
            info!("telegram disabled; skipping bot setup");
            TelegramBridge::disabled()
        };
        let (codex_events_tx, codex_events_rx) = mpsc::channel(256);
        let (app_control_tx, app_control_rx) = mpsc::channel(128);
        let app_gateway = if config.app.enabled {
            Some(AppGatewayHandle::spawn(
                config.app.clone(),
                auth_store.clone(),
                app_control_tx,
            )?)
        } else {
            None
        };

        let mut app = Self {
            config,
            telegram,
            state_store,
            state,
            runtimes: HashMap::new(),
            codex_events_tx,
            codex_events_rx,
            app_control_rx,
            app_gateway,
            update_offset: 0,
            active_runs: HashMap::new(),
            active_file_changes: HashMap::new(),
        };

        if let Some(active_repo_id) = app.state.active_repo_id.clone() {
            if let Err(err) = app
                .ensure_runtime_for_repo(&active_repo_id, None, ThreadSurface::Telegram)
                .await
            {
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

        if config.telegram.is_enabled() {
            let telegram = TelegramBridge::enabled(&config.telegram.bot_token);
            let user = telegram.get_me().await.context("telegram getMe failed")?;
            info!(
                "telegram token valid for bot {}",
                user.username.unwrap_or(user.first_name)
            );
        } else {
            info!("telegram disabled; skipping Telegram connectivity check");
        }

        probe_codex(&config).await?;
        let discovered = discover_workspace_repos(&config.workspace.root)?;
        info!("workspace scan found {} repo(s)", discovered.len());
        Ok(())
    }

    pub async fn run(&mut self) -> Result<()> {
        info!("starting MyCodex event loop");
        let mut runtime_poll = interval(Duration::from_millis(800));
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
                maybe_command = self.app_control_rx.recv() => {
                    match maybe_command {
                        Some(command) => self.handle_app_control_command(command).await,
                        None => break,
                    }
                }
                _ = runtime_poll.tick() => {
                    self.reconcile_runtime_exits(None).await?;
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
        self.reconcile_runtime_exits(chat_id).await?;

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
        if !self.config.telegram.is_enabled() {
            return MessageAccess::Denied;
        }
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
        if !self.config.telegram.is_enabled() {
            return false;
        }
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
                let active_repo_id = self.state.active_repo_id.as_deref();
                let active_run = active_repo_id.and_then(|repo_id| self.active_runs.get(repo_id));
                let approval_rule_count = self
                    .state
                    .active_repo_id
                    .as_deref()
                    .map(|repo_id| self.state.approval_rules_for_repo(repo_id).len())
                    .unwrap_or(0);
                let body = render_status(
                    self.state.active_repo(),
                    self.state.active_thread(),
                    active_repo_id.filter(|repo_id| self.runtimes.contains_key(*repo_id)),
                    active_run.map(|run| run.turn_id.as_str()),
                    active_run
                        .and_then(|run| run.pending_request.as_ref())
                        .map(pending_request_summary)
                        .as_deref(),
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
        if matches!(command, ApprovalCommand::Menu) {
            self.telegram
                .send_message(
                    chat_id,
                    &render_approval_menu(),
                    Some(&approval_menu_keyboard()),
                )
                .await?;
            return Ok(());
        }

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
            ApprovalCommand::Menu => unreachable!("menu command returned early"),
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
            RepoCommand::Menu => {
                self.telegram
                    .send_message(chat_id, &render_repo_menu(), Some(&repo_menu_keyboard()))
                    .await?;
            }
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
                if self
                    .state
                    .active_repo_id
                    .as_ref()
                    .is_some_and(|repo_id| self.active_runs.contains_key(repo_id))
                {
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
                if self
                    .state
                    .active_repo_id
                    .as_ref()
                    .is_some_and(|repo_id| self.active_runs.contains_key(repo_id))
                {
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
        if matches!(command, ThreadCommand::Menu) {
            self.telegram
                .send_message(
                    chat_id,
                    &render_thread_menu(),
                    Some(&thread_menu_keyboard()),
                )
                .await?;
            return Ok(());
        }

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
            ThreadCommand::Menu => unreachable!("menu command returned early"),
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
                if self.active_runs.contains_key(&active_repo_id) {
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
                let runtime = self
                    .ensure_runtime_for_repo(&active_repo_id, None, ThreadSurface::Telegram)
                    .await?;
                let response = runtime.create_thread(model).await?;
                let title = format!("Thread {}", Utc::now().format("%Y-%m-%d %H:%M"));
                let thread = self.state.create_thread_for_repo(
                    &active_repo_id,
                    response.thread.id,
                    response.thread.path,
                    title,
                    false,
                    ThreadSurface::Telegram,
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
                if self.active_runs.contains_key(&active_repo_id) {
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
                    .resolve_thread_ref_for_surface(&repo, &thread, ThreadSurface::Telegram)
                    .cloned()
                    .context("thread not found")?;
                self.use_repo(active_repo_id.clone()).await?;
                let model = self.config.codex.model.clone();
                let runtime = self
                    .ensure_runtime_for_repo(
                        &active_repo_id,
                        Some(&thread),
                        ThreadSurface::Telegram,
                    )
                    .await?;
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
                    ThreadSurface::Telegram,
                    response.thread.id,
                    response.thread.path,
                )?;
                self.state.activate_thread(
                    &active_repo_id,
                    &thread.local_thread_id,
                    ThreadSurface::Telegram,
                )?;
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

    async fn handle_menu_callback(
        &mut self,
        callback: CallbackQuery,
        action: MenuAction,
    ) -> Result<()> {
        let message = callback
            .message
            .as_ref()
            .context("menu callback missing message")?;
        let chat_id = message.chat.id;
        let message_id = message.message_id;

        match action {
            MenuAction::RepoRoot => {
                self.update_menu_message(
                    chat_id,
                    message_id,
                    &render_repo_menu(),
                    Some(&repo_menu_keyboard()),
                )
                .await?;
                self.telegram
                    .answer_callback_query(&callback.id, None)
                    .await?;
            }
            MenuAction::RepoList => {
                self.telegram
                    .answer_callback_query(&callback.id, Some("Listing repos."))
                    .await?;
                self.handle_repo_command(chat_id, RepoCommand::List).await?;
            }
            MenuAction::RepoUseMenu => {
                self.update_menu_message(
                    chat_id,
                    message_id,
                    &render_repo_use_menu(&self.state.repos),
                    Some(&repo_use_keyboard(&self.state.repos)),
                )
                .await?;
                self.telegram
                    .answer_callback_query(&callback.id, None)
                    .await?;
            }
            MenuAction::RepoUse { repo_id } => {
                self.handle_repo_command(chat_id, RepoCommand::Use { repo: repo_id })
                    .await?;
                self.update_menu_message(
                    chat_id,
                    message_id,
                    &render_repo_menu(),
                    Some(&repo_menu_keyboard()),
                )
                .await?;
                self.telegram
                    .answer_callback_query(&callback.id, Some("Repo switched."))
                    .await?;
            }
            MenuAction::RepoCloneHelp => {
                self.update_menu_message(
                    chat_id,
                    message_id,
                    &render_repo_clone_menu(),
                    Some(&back_keyboard("menu:repo")),
                )
                .await?;
                self.telegram
                    .answer_callback_query(&callback.id, None)
                    .await?;
            }
            MenuAction::RepoStatus => {
                self.telegram
                    .answer_callback_query(&callback.id, Some("Showing repo status."))
                    .await?;
                self.handle_repo_command(chat_id, RepoCommand::Status)
                    .await?;
            }
            MenuAction::RepoRescan => {
                self.telegram
                    .answer_callback_query(&callback.id, Some("Rescanning workspace."))
                    .await?;
                self.handle_repo_command(chat_id, RepoCommand::Rescan)
                    .await?;
            }
            MenuAction::ThreadRoot => {
                self.update_menu_message(
                    chat_id,
                    message_id,
                    &render_thread_menu(),
                    Some(&thread_menu_keyboard()),
                )
                .await?;
                self.telegram
                    .answer_callback_query(&callback.id, None)
                    .await?;
            }
            MenuAction::ThreadList => {
                self.telegram
                    .answer_callback_query(&callback.id, Some("Listing threads."))
                    .await?;
                self.handle_thread_command(chat_id, ThreadCommand::List)
                    .await?;
            }
            MenuAction::ThreadNew => {
                self.handle_thread_command(chat_id, ThreadCommand::New)
                    .await?;
                self.update_menu_message(
                    chat_id,
                    message_id,
                    &render_thread_menu(),
                    Some(&thread_menu_keyboard()),
                )
                .await?;
                self.telegram
                    .answer_callback_query(&callback.id, Some("Thread created."))
                    .await?;
            }
            MenuAction::ThreadUseMenu => {
                let (text, keyboard) = match self.state.active_repo_id.as_deref() {
                    Some(repo_id) => {
                        let repo = self
                            .state
                            .find_repo_by_id(repo_id)
                            .context("active repo missing")?;
                        (render_thread_use_menu(repo), thread_use_keyboard(repo))
                    }
                    None => (
                        "No active repo. Use /repo use or /repo clone first.".to_string(),
                        back_keyboard("menu:thread"),
                    ),
                };
                self.update_menu_message(chat_id, message_id, &text, Some(&keyboard))
                    .await?;
                self.telegram
                    .answer_callback_query(&callback.id, None)
                    .await?;
            }
            MenuAction::ThreadUse { thread_id } => {
                self.handle_thread_command(chat_id, ThreadCommand::Use { thread: thread_id })
                    .await?;
                self.update_menu_message(
                    chat_id,
                    message_id,
                    &render_thread_menu(),
                    Some(&thread_menu_keyboard()),
                )
                .await?;
                self.telegram
                    .answer_callback_query(&callback.id, Some("Thread switched."))
                    .await?;
            }
            MenuAction::ThreadStatus => {
                self.telegram
                    .answer_callback_query(&callback.id, Some("Showing thread status."))
                    .await?;
                self.handle_thread_command(chat_id, ThreadCommand::Status)
                    .await?;
            }
            MenuAction::ApprovalRoot => {
                self.update_menu_message(
                    chat_id,
                    message_id,
                    &render_approval_menu(),
                    Some(&approval_menu_keyboard()),
                )
                .await?;
                self.telegram
                    .answer_callback_query(&callback.id, None)
                    .await?;
            }
            MenuAction::ApprovalList => {
                self.telegram
                    .answer_callback_query(&callback.id, Some("Listing approval rules."))
                    .await?;
                self.handle_approval_command(chat_id, ApprovalCommand::List)
                    .await?;
            }
            MenuAction::ApprovalRemoveMenu => {
                let (text, keyboard) = match self.state.active_repo() {
                    Some(repo) => {
                        let rules = self.state.approval_rules_for_repo(&repo.repo_id);
                        (
                            render_approval_remove_menu(repo, &rules),
                            approval_remove_keyboard(&rules),
                        )
                    }
                    None => (
                        "No active repo. Use /repo use or /repo clone first.".to_string(),
                        back_keyboard("menu:approval"),
                    ),
                };
                self.update_menu_message(chat_id, message_id, &text, Some(&keyboard))
                    .await?;
                self.telegram
                    .answer_callback_query(&callback.id, None)
                    .await?;
            }
            MenuAction::ApprovalRemove { rule_id } => {
                self.handle_approval_command(chat_id, ApprovalCommand::Remove { rule: rule_id })
                    .await?;
                self.update_menu_message(
                    chat_id,
                    message_id,
                    &render_approval_menu(),
                    Some(&approval_menu_keyboard()),
                )
                .await?;
                self.telegram
                    .answer_callback_query(&callback.id, Some("Approval rule removed."))
                    .await?;
            }
            MenuAction::ApprovalClear => {
                self.handle_approval_command(chat_id, ApprovalCommand::Clear)
                    .await?;
                self.update_menu_message(
                    chat_id,
                    message_id,
                    &render_approval_menu(),
                    Some(&approval_menu_keyboard()),
                )
                .await?;
                self.telegram
                    .answer_callback_query(&callback.id, Some("Approval rules cleared."))
                    .await?;
            }
        }

        Ok(())
    }

    async fn update_menu_message(
        &self,
        chat_id: i64,
        message_id: i64,
        text: &str,
        keyboard: Option<&InlineKeyboardMarkup>,
    ) -> Result<()> {
        match self
            .telegram
            .edit_message_text(chat_id, message_id, text, keyboard)
            .await
        {
            Ok(_) => Ok(()),
            Err(err) if err.to_string().contains("message is not modified") => Ok(()),
            Err(err) => Err(err),
        }
    }

    async fn handle_text(&mut self, chat_id: i64, text: String) -> Result<()> {
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
        if self.active_runs.contains_key(&repo_id) {
            self.telegram
                .send_message(
                    chat_id,
                    "This repo already has a running turn. Wait for it or use /abort.",
                    None,
                )
                .await?;
            return Ok(());
        }

        self.use_repo(repo_id.clone()).await?;

        let mut active_thread = if let Some(thread) = self.state.active_thread().cloned() {
            thread
        } else {
            let model = self.config.codex.model.clone();
            let response = self
                .ensure_runtime_for_repo(&repo_id, None, ThreadSurface::Telegram)
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
                ThreadSurface::Telegram,
            )?;
            self.persist_state()?;
            thread
        };

        if !active_thread.has_user_message {
            let title = title_from_text(&text);
            self.state.update_thread_title(
                &repo_id,
                &active_thread.local_thread_id,
                ThreadSurface::Telegram,
                title.clone(),
            )?;
            self.persist_state()?;
            active_thread.title = title;
            active_thread.has_user_message = true;
        }

        let model = self.config.codex.model.clone();
        let network_access = self.config.codex.network_access;
        let response = self
            .ensure_runtime_for_repo(&repo_id, Some(&active_thread), ThreadSurface::Telegram)
            .await?
            .start_turn(
                active_thread.codex_thread_id.clone(),
                text,
                model,
                network_access,
            )
            .await?;

        let progress = self
            .telegram
            .send_message(chat_id, "Starting Codex turn...", None)
            .await?;
        self.active_file_changes.remove(&repo_id);
        self.active_runs.insert(
            repo_id.clone(),
            ActiveRun {
                surface: ThreadSurface::Telegram,
                thread_local_id: active_thread.local_thread_id,
                turn_id: response.turn.id,
                assistant_text: String::new(),
                command_output_tail: String::new(),
                diff_preview: String::new(),
                route: RunRoute::Telegram {
                    chat_id,
                    message_id: progress.message_id,
                    last_rendered_at: None,
                },
                pending_request: None,
            },
        );
        self.render_progress_for_repo(&repo_id, false).await?;
        Ok(())
    }

    async fn handle_callback(&mut self, callback: CallbackQuery) -> Result<()> {
        let data = callback.data.clone().unwrap_or_default();
        if let Some(action) = parse_menu_action(&data) {
            return self.handle_menu_callback(callback, action).await;
        }
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
        let Some((repo_id, pending)) = self.find_pending_request(&callback_request_id) else {
            self.mark_callback_inactive(&callback, "no longer active")
                .await?;
            return Ok(());
        };

        match (pending, action.as_str()) {
            (RuntimePendingRequest::Command(pending), "cmd:approve") => {
                self.ensure_runtime_for_repo(&repo_id, None, ThreadSurface::Telegram)
                    .await?
                    .respond_command_approval(
                        pending.request_id,
                        CommandExecutionApprovalDecision::Accept,
                    )
                    .await?;
                self.telegram
                    .answer_callback_query(&callback.id, Some("Command approved."))
                    .await?;
                self.cleanup_pending_request(&repo_id, Some("approved"))
                    .await?;
            }
            (RuntimePendingRequest::Command(pending), "cmd:allow") => {
                let Some(command) = pending.command else {
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
                self.ensure_runtime_for_repo(&repo_id, None, ThreadSurface::Telegram)
                    .await?
                    .respond_command_approval(
                        pending.request_id,
                        CommandExecutionApprovalDecision::Accept,
                    )
                    .await?;
                self.telegram
                    .answer_callback_query(&callback.id, Some("Command approved and saved."))
                    .await?;
                self.cleanup_pending_request(
                    &repo_id,
                    Some(&format!("approved and saved [{}]", short_id(&rule.rule_id))),
                )
                .await?;
            }
            (RuntimePendingRequest::Command(pending), "cmd:decline") => {
                self.ensure_runtime_for_repo(&repo_id, None, ThreadSurface::Telegram)
                    .await?
                    .respond_command_approval(
                        pending.request_id,
                        CommandExecutionApprovalDecision::Decline,
                    )
                    .await?;
                self.telegram
                    .answer_callback_query(&callback.id, Some("Command declined."))
                    .await?;
                self.cleanup_pending_request(&repo_id, Some("declined"))
                    .await?;
            }
            (RuntimePendingRequest::Command(pending), "cmd:abort") => {
                self.ensure_runtime_for_repo(&repo_id, None, ThreadSurface::Telegram)
                    .await?
                    .respond_command_approval(
                        pending.request_id,
                        CommandExecutionApprovalDecision::Cancel,
                    )
                    .await?;
                self.telegram
                    .answer_callback_query(&callback.id, Some("Turn abort requested."))
                    .await?;
                self.cleanup_pending_request(&repo_id, Some("abort requested"))
                    .await?;
            }
            (RuntimePendingRequest::File(pending), "patch:approve") => {
                self.ensure_runtime_for_repo(&repo_id, None, ThreadSurface::Telegram)
                    .await?
                    .respond_file_approval(pending.request_id, FileChangeApprovalDecision::Accept)
                    .await?;
                self.telegram
                    .answer_callback_query(&callback.id, Some("Patch approved."))
                    .await?;
                self.cleanup_pending_request(&repo_id, Some("approved"))
                    .await?;
            }
            (RuntimePendingRequest::File(pending), "patch:decline") => {
                self.ensure_runtime_for_repo(&repo_id, None, ThreadSurface::Telegram)
                    .await?
                    .respond_file_approval(pending.request_id, FileChangeApprovalDecision::Decline)
                    .await?;
                self.telegram
                    .answer_callback_query(&callback.id, Some("Patch declined."))
                    .await?;
                self.cleanup_pending_request(&repo_id, Some("declined"))
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
                let mut app_target = None;
                if let Some(run) = self.active_runs.get_mut(&repo_id) {
                    run.turn_id = payload.turn.id.clone();
                    if let RunRoute::App { device_id } = &run.route {
                        app_target = Some((device_id.clone(), run.thread_local_id.clone()));
                    }
                }
                if let Some((device_id, thread_id)) = app_target {
                    self.emit_app_event(
                        &device_id,
                        "run.started",
                        serde_json::json!({
                            "repo_id": repo_id,
                            "thread_id": thread_id,
                            "turn_id": payload.turn.id,
                        }),
                    )
                    .await;
                }
            }
            CodexEvent::TurnCompleted { repo_id, payload } => {
                if self
                    .active_runs
                    .get(&repo_id)
                    .map(|run| run.turn_id.as_str())
                    != Some(payload.turn.id.as_str())
                {
                    return Ok(());
                }
                self.finish_turn(
                    &repo_id,
                    &payload.turn.status,
                    payload
                        .turn
                        .error
                        .as_ref()
                        .map(|error| error.message.as_str()),
                )
                .await?;
            }
            CodexEvent::AgentMessageDelta { repo_id, payload } => {
                let delta = payload.delta;
                let mut app_target = None;
                let mut should_render = false;
                if let Some(run) = self.active_runs.get_mut(&repo_id) {
                    run.assistant_text.push_str(&delta);
                    match &run.route {
                        RunRoute::Telegram { .. } => should_render = true,
                        RunRoute::App { device_id } => {
                            app_target = Some((
                                device_id.clone(),
                                run.thread_local_id.clone(),
                                run.turn_id.clone(),
                                run.assistant_text.clone(),
                            ));
                        }
                    }
                }
                if should_render {
                    self.render_progress_for_repo(&repo_id, false).await?;
                }
                if let Some((device_id, thread_id, turn_id, assistant_text)) = app_target {
                    self.emit_app_event(
                        &device_id,
                        "run.delta",
                        serde_json::json!({
                            "repo_id": repo_id,
                            "thread_id": thread_id,
                            "turn_id": turn_id,
                            "delta": delta,
                            "assistant_text": assistant_text,
                        }),
                    )
                    .await;
                }
            }
            CodexEvent::CommandOutputDelta { repo_id, payload } => {
                let delta = payload.delta;
                let mut app_target = None;
                let mut should_render = false;
                if let Some(run) = self.active_runs.get_mut(&repo_id) {
                    run.command_output_tail.push_str(&delta);
                    run.command_output_tail = trim_output_tail(&run.command_output_tail, 4_000);
                    match &run.route {
                        RunRoute::Telegram { .. } => should_render = true,
                        RunRoute::App { device_id } => {
                            app_target = Some((
                                device_id.clone(),
                                run.thread_local_id.clone(),
                                run.turn_id.clone(),
                                run.command_output_tail.clone(),
                            ));
                        }
                    }
                }
                if should_render {
                    self.render_progress_for_repo(&repo_id, false).await?;
                }
                if let Some((device_id, thread_id, turn_id, command_output_tail)) = app_target {
                    self.emit_app_event(
                        &device_id,
                        "run.command_output",
                        serde_json::json!({
                            "repo_id": repo_id,
                            "thread_id": thread_id,
                            "turn_id": turn_id,
                            "delta": delta,
                            "command_output_tail": command_output_tail,
                        }),
                    )
                    .await;
                }
            }
            CodexEvent::DiffUpdated { repo_id, payload } => {
                let diff = payload.diff;
                let mut app_target = None;
                let mut should_render = false;
                if let Some(run) = self.active_runs.get_mut(&repo_id) {
                    run.diff_preview = diff.clone();
                    match &run.route {
                        RunRoute::Telegram { .. } => should_render = true,
                        RunRoute::App { device_id } => {
                            app_target = Some((
                                device_id.clone(),
                                run.thread_local_id.clone(),
                                run.turn_id.clone(),
                                run.diff_preview.clone(),
                            ));
                        }
                    }
                }
                if should_render {
                    self.render_progress_for_repo(&repo_id, false).await?;
                }
                if let Some((device_id, thread_id, turn_id, diff_preview)) = app_target {
                    self.emit_app_event(
                        &device_id,
                        "run.diff",
                        serde_json::json!({
                            "repo_id": repo_id,
                            "thread_id": thread_id,
                            "turn_id": turn_id,
                            "diff_preview": diff_preview,
                        }),
                    )
                    .await;
                }
            }
            CodexEvent::ItemStarted { repo_id, payload } => {
                if let ThreadItem::FileChange { id, changes, .. } = payload.item {
                    self.active_file_changes
                        .entry(repo_id)
                        .or_default()
                        .insert(id, changes);
                }
            }
            CodexEvent::ItemCompleted { repo_id, payload } => {
                if let ThreadItem::FileChange { id, .. } = payload.item {
                    if let Some(items) = self.active_file_changes.get_mut(&repo_id) {
                        items.remove(&id);
                    }
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
                if self
                    .active_runs
                    .get(&repo_id)
                    .and_then(|run| run.pending_request.as_ref())
                    .map(|pending| pending_request_id(pending) == &payload.request_id)
                    .unwrap_or(false)
                {
                    self.cleanup_pending_request(&repo_id, Some("processed"))
                        .await?;
                }
            }
            CodexEvent::Error { repo_id, payload } => {
                warn!(
                    "codex turn error thread={} turn={} retry={}: {}",
                    payload.thread_id, payload.turn_id, payload.will_retry, payload.error.message
                );
                if !payload.will_retry
                    && self
                        .active_runs
                        .get(&repo_id)
                        .map(|run| run.turn_id.as_str())
                        == Some(payload.turn_id.as_str())
                {
                    self.finish_turn(
                        &repo_id,
                        &TurnStatus::Failed,
                        Some(payload.error.message.as_str()),
                    )
                    .await?;
                }
            }
            CodexEvent::RuntimeExited {
                repo_id,
                status_code,
                error,
            } => {
                self.on_runtime_exited(&repo_id, status_code, error.as_deref(), None)
                    .await?;
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
        let run = self
            .active_runs
            .get(&repo_id)
            .cloned()
            .context("approval run missing")?;
        let thread = self
            .state
            .find_thread_for_surface(&repo_id, &run.thread_local_id, run.surface)
            .cloned()
            .context("approval thread missing")?;
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
                self.ensure_runtime_for_repo(&repo_id, None, run.surface)
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
        if run.pending_request.is_some() {
            self.cleanup_pending_request(&repo_id, Some("superseded by a newer approval"))
                .await?;
        }

        let telegram_message = match &run.route {
            RunRoute::Telegram { chat_id, .. } => {
                let approval_message = self
                    .telegram
                    .send_message(
                        *chat_id,
                        &body,
                        Some(&command_approval_keyboard(&request_id)),
                    )
                    .await?;
                Some(TelegramApprovalMessage {
                    chat_id: *chat_id,
                    message_id: approval_message.message_id,
                    text: body.clone(),
                })
            }
            RunRoute::App { device_id } => {
                self.emit_app_event(
                    device_id,
                    "run.approval_required",
                    serde_json::json!({
                        "repo_id": repo_id.clone(),
                        "thread_id": thread.local_thread_id.clone(),
                        "turn_id": params.turn_id.clone(),
                        "request_id": request_id.clone(),
                        "kind": "command",
                        "thread_title": thread.title.clone(),
                        "command": params.command.clone(),
                        "cwd": params.cwd.clone(),
                        "reason": params.reason.clone(),
                    }),
                )
                .await;
                None
            }
        };

        if let Some(run) = self.active_runs.get_mut(&repo_id) {
            run.pending_request = Some(RuntimePendingRequest::Command(CommandPendingRequest {
                request_id,
                thread_title: thread.title,
                turn_id: params.turn_id,
                item_id: params.item_id,
                command: params.command,
                cwd: params.cwd,
                reason: params.reason,
                telegram_message,
            }));
        }
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
        let run = self
            .active_runs
            .get(&repo_id)
            .cloned()
            .context("approval run missing")?;
        let thread = self
            .state
            .find_thread_for_surface(&repo_id, &run.thread_local_id, run.surface)
            .cloned()
            .context("approval thread missing")?;
        let changes = self
            .active_file_changes
            .get(&repo_id)
            .and_then(|items| items.get(&params.item_id))
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
        if run.pending_request.is_some() {
            self.cleanup_pending_request(&repo_id, Some("superseded by a newer approval"))
                .await?;
        }

        let caption = render_file_approval(
            &repo.name,
            &thread.title,
            &paths,
            params.reason.as_deref(),
            &diff_preview,
        );
        let (patch_path, telegram_message) = match &run.route {
            RunRoute::Telegram { chat_id, .. } => {
                let patch_path = if diff_preview.len() > self.config.ui.max_inline_diff_chars {
                    let path = self
                        .config
                        .temp_dir()
                        .join(format!("{}.patch", params.item_id.replace('/', "_")));
                    tokio::fs::write(&path, diff_preview.as_bytes())
                        .await
                        .with_context(|| format!("failed to write {}", path.display()))?;
                    self.telegram
                        .send_document(*chat_id, &path, &caption)
                        .await?;
                    Some(path)
                } else {
                    None
                };
                let prompt = if patch_path.is_some() {
                    "Use the buttons below to approve or decline this patch.".to_string()
                } else {
                    caption.clone()
                };
                let message = self
                    .telegram
                    .send_message(
                        *chat_id,
                        &prompt,
                        Some(&file_approval_keyboard(&request_id)),
                    )
                    .await?;
                (
                    patch_path,
                    Some(TelegramApprovalMessage {
                        chat_id: *chat_id,
                        message_id: message.message_id,
                        text: prompt,
                    }),
                )
            }
            RunRoute::App { device_id } => {
                self.emit_app_event(
                    device_id,
                    "run.approval_required",
                    serde_json::json!({
                        "repo_id": repo_id.clone(),
                        "thread_id": thread.local_thread_id.clone(),
                        "turn_id": params.turn_id.clone(),
                        "request_id": request_id.clone(),
                        "kind": "file",
                        "thread_title": thread.title.clone(),
                        "paths": paths.clone(),
                        "reason": params.reason.clone(),
                        "diff_preview": diff_preview.clone(),
                    }),
                )
                .await;
                (None, None)
            }
        };

        if let Some(run) = self.active_runs.get_mut(&repo_id) {
            run.pending_request = Some(RuntimePendingRequest::File(FilePendingRequest {
                request_id,
                thread_title: thread.title,
                turn_id: params.turn_id,
                item_id: params.item_id,
                paths,
                reason: params.reason,
                diff_preview,
                patch_path,
                preferred_decision: FileChangeApprovalDecision::Accept,
                telegram_message,
            }));
        }
        Ok(())
    }

    async fn finish_turn(
        &mut self,
        repo_id: &str,
        status: &TurnStatus,
        error_message: Option<&str>,
    ) -> Result<()> {
        let Some(run) = self.active_runs.get(repo_id).cloned() else {
            return Ok(());
        };
        let final_text = if run.assistant_text.is_empty() {
            status_label(status).to_string()
        } else {
            run.assistant_text.clone()
        };

        if matches!(run.route, RunRoute::Telegram { .. }) {
            self.render_progress_for_repo(repo_id, true).await?;
        }

        match &run.route {
            RunRoute::Telegram {
                chat_id,
                message_id,
                ..
            } => {
                let chunks = split_message(&final_text);
                if chunks.len() > 1 {
                    let repo_name = self
                        .state
                        .find_repo_by_id(repo_id)
                        .map(|repo| repo.name.clone())
                        .unwrap_or_else(|| repo_id.to_string());
                    let thread_title = self
                        .state
                        .find_thread_for_surface(repo_id, &run.thread_local_id, run.surface)
                        .map(|thread| thread.title.clone())
                        .unwrap_or_else(|| run.thread_local_id.clone());
                    let summary = format!(
                        "repo: {repo_name}\nthread: {thread_title}\nstatus: {}\n\nFull response sent in {} parts.",
                        status_label(status),
                        chunks.len()
                    );
                    let _ = self
                        .telegram
                        .edit_message_text(*chat_id, *message_id, &summary, None)
                        .await;
                    for chunk in chunks {
                        self.telegram.send_message(*chat_id, &chunk, None).await?;
                    }
                }
                if let Some(error_message) = error_message {
                    self.telegram
                        .send_message(
                            *chat_id,
                            &format!("Turn ended with error: {error_message}"),
                            None,
                        )
                        .await?;
                }
            }
            RunRoute::App { device_id } => {
                let method = if matches!(status, TurnStatus::Failed | TurnStatus::Interrupted) {
                    "run.failed"
                } else {
                    "run.completed"
                };
                self.emit_app_event(
                    device_id,
                    method,
                    serde_json::json!({
                        "repo_id": repo_id,
                        "thread_id": run.thread_local_id,
                        "turn_id": run.turn_id,
                        "status": status_label(status),
                        "assistant_text": final_text,
                        "command_output_tail": run.command_output_tail,
                        "diff_preview": run.diff_preview,
                        "error": error_message,
                    }),
                )
                .await;
            }
        }

        self.cleanup_pending_request(repo_id, Some("no longer active"))
            .await?;
        self.active_runs.remove(repo_id);
        self.active_file_changes.remove(repo_id);
        Ok(())
    }

    async fn abort_active_turn(&mut self, chat_id: i64) -> Result<()> {
        let repo_id = match self.state.active_repo_id.clone() {
            Some(repo_id) => repo_id,
            None => {
                self.telegram
                    .send_message(chat_id, "No active repo is selected.", None)
                    .await?;
                return Ok(());
            }
        };
        let run = match self.active_runs.get(&repo_id).cloned() {
            Some(run) if run.surface == ThreadSurface::Telegram => run,
            _ => {
                self.telegram
                    .send_message(chat_id, "No active turn is running.", None)
                    .await?;
                return Ok(());
            }
        };
        let thread = self
            .state
            .find_thread_for_surface(&repo_id, &run.thread_local_id, ThreadSurface::Telegram)
            .cloned()
            .context("active thread missing")?;
        self.ensure_runtime_for_repo(&repo_id, None, ThreadSurface::Telegram)
            .await?
            .interrupt_turn(thread.codex_thread_id, run.turn_id)
            .await?;
        self.telegram
            .send_message(chat_id, "Turn interrupt requested.", None)
            .await?;
        Ok(())
    }

    async fn render_progress_for_repo(&mut self, repo_id: &str, force: bool) -> Result<()> {
        let Some(run) = self.active_runs.get(repo_id) else {
            return Ok(());
        };
        let (chat_id, message_id, last_rendered_at) = match &run.route {
            RunRoute::Telegram {
                chat_id,
                message_id,
                last_rendered_at,
            } => (*chat_id, *message_id, *last_rendered_at),
            RunRoute::App { .. } => return Ok(()),
        };

        let should_render = force
            || last_rendered_at
                .map(|last| {
                    last.elapsed() >= Duration::from_millis(self.config.ui.stream_edit_interval_ms)
                })
                .unwrap_or(true);
        if !should_render {
            return Ok(());
        }

        let repo = self
            .state
            .find_repo_by_id(repo_id)
            .context("progress repo missing")?;
        let thread = self
            .state
            .find_thread_for_surface(repo_id, &run.thread_local_id, run.surface)
            .context("progress thread missing")?;
        let view = ProgressView {
            repo_name: repo.name.clone(),
            thread_title: thread.title.clone(),
            status: status_label_from_state(Some(run.turn_id.as_str()), force),
            assistant_text: run.assistant_text.clone(),
            command_output_tail: run.command_output_tail.clone(),
            diff_preview: run.diff_preview.clone(),
        };

        let rendered = render_progress(&view);
        self.telegram
            .edit_message_text(chat_id, message_id, &rendered, None)
            .await?;
        if let Some(ActiveRun {
            route: RunRoute::Telegram {
                last_rendered_at, ..
            },
            ..
        }) = self.active_runs.get_mut(repo_id)
        {
            *last_rendered_at = Some(Instant::now());
        }
        Ok(())
    }

    async fn cleanup_pending_request(&mut self, repo_id: &str, status: Option<&str>) -> Result<()> {
        let pending = self
            .active_runs
            .get(repo_id)
            .and_then(|run| run.pending_request.clone());
        if let Some(pending) = pending {
            if let Some(status) = status {
                if let Err(err) = self.update_pending_request_message(&pending, status).await {
                    warn!("failed to update pending approval message: {err}");
                }
            }
            if let RuntimePendingRequest::File(FilePendingRequest {
                patch_path: Some(path),
                ..
            }) = &pending
            {
                let _ = tokio::fs::remove_file(path).await;
            }
        }
        if let Some(run) = self.active_runs.get_mut(repo_id) {
            run.pending_request = None;
        }
        Ok(())
    }

    async fn reconcile_runtime_exits(&mut self, chat_id: Option<i64>) -> Result<()> {
        let mut exited = Vec::new();
        for (repo_id, runtime) in &mut self.runtimes {
            if let Some(status) = runtime.try_wait()? {
                exited.push((repo_id.clone(), status.code()));
            }
        }
        for (repo_id, status_code) in exited {
            self.on_runtime_exited(&repo_id, status_code, None, chat_id)
                .await?;
        }
        Ok(())
    }

    async fn on_runtime_exited(
        &mut self,
        repo_id: &str,
        status_code: Option<i32>,
        error: Option<&str>,
        chat_id: Option<i64>,
    ) -> Result<()> {
        if let Some(runtime) = self.runtimes.remove(repo_id) {
            let _ = runtime.stop().await;
        }
        let repo_name = self
            .state
            .find_repo_by_id(repo_id)
            .map(|repo| repo.name.clone())
            .unwrap_or_else(|| repo_id.to_string());
        let runtime_error = error
            .map(|value| value.to_string())
            .or_else(|| status_code.map(|code| format!("status {code}")));
        let run_snapshot = self.active_runs.get(repo_id).cloned();
        let had_pending = run_snapshot
            .as_ref()
            .and_then(|run| run.pending_request.as_ref())
            .is_some();
        if let Some(run) = run_snapshot.as_ref() {
            match &run.route {
                RunRoute::Telegram {
                    chat_id,
                    message_id,
                    ..
                } => {
                    let thread_title = self
                        .state
                        .find_thread_for_surface(repo_id, &run.thread_local_id, run.surface)
                        .map(|thread| thread.title.clone())
                        .unwrap_or_else(|| run.thread_local_id.clone());
                    let _ = self
                        .telegram
                        .edit_message_text(
                            *chat_id,
                            *message_id,
                            &format!(
                                "repo: {repo_name}\nthread: {thread_title}\nstatus: runtime exited"
                            ),
                            None,
                        )
                        .await;
                }
                RunRoute::App { device_id } => {
                    self.emit_app_event(
                        device_id,
                        "run.failed",
                        serde_json::json!({
                            "repo_id": repo_id,
                            "thread_id": run.thread_local_id,
                            "turn_id": run.turn_id,
                            "status": "runtime exited",
                            "error": runtime_error,
                        }),
                    )
                    .await;
                }
            }
        }

        if had_pending {
            self.cleanup_pending_request(repo_id, Some("no longer active"))
                .await?;
        }
        self.active_runs.remove(repo_id);
        self.active_file_changes.remove(repo_id);

        warn!(
            "codex runtime exited for repo {} status={:?} error={:?}",
            repo_id, status_code, error
        );
        if let Some(RunRoute::Telegram {
            chat_id: progress_chat,
            ..
        }) = run_snapshot.as_ref().map(|run| run.route.clone())
        {
            let notify_chat_id = chat_id
                .or(Some(progress_chat))
                .or(self.config.telegram.allowed_chat_id);
            if let Some(chat_id) = notify_chat_id {
                let status = status_code
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
        pending: &RuntimePendingRequest,
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
        self.state.set_active_repo(repo_id.clone());
        self.state.mark_repo_used(&repo_id);
        self.persist_state()?;
        self.ensure_runtime_for_repo(&repo_id, None, ThreadSurface::Telegram)
            .await?;
        Ok(())
    }

    async fn ensure_runtime_for_repo(
        &mut self,
        repo_id: &str,
        startup_thread: Option<&ThreadRecord>,
        surface: ThreadSurface,
    ) -> Result<&mut CodexRuntime> {
        if self.runtimes.contains_key(repo_id) {
            return self
                .runtimes
                .get_mut(repo_id)
                .context("runtime map missing requested repo");
        }

        let repo = self
            .state
            .find_repo_by_id(repo_id)
            .cloned()
            .with_context(|| format!("repo not found: {repo_id}"))?;
        let mut runtime = CodexRuntime::start(
            &self.config.codex.bin,
            repo.repo_id.clone(),
            repo.path.clone(),
            self.codex_events_tx.clone(),
        )
        .await?;

        let resume_thread = startup_thread.cloned().or_else(|| {
            (surface == ThreadSurface::Telegram)
                .then(|| repo.active_thread().cloned())
                .flatten()
        });
        if let Some(thread) = resume_thread {
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
                    if surface == ThreadSurface::Telegram && is_missing_thread_resume_error(&err) {
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
                        self.persist_state()?;
                        self.runtimes.insert(repo_id.to_string(), runtime);
                        return self
                            .runtimes
                            .get_mut(repo_id)
                            .context("runtime must exist after startup");
                    }
                    let _ = runtime.stop().await;
                    return Err(err).with_context(|| {
                        format!(
                            "failed to resume thread {} [{}] for repo {}",
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
                thread.surface,
                response.thread.id,
                response.thread.path,
            )?;
            self.persist_state()?;
        }

        self.runtimes.insert(repo_id.to_string(), runtime);
        self.runtimes
            .get_mut(repo_id)
            .context("runtime must exist after startup")
    }

    async fn handle_app_control_command(&mut self, command: AppControlCommand) {
        match command {
            AppControlCommand::ListRepos { reply, .. } => {
                let result = serde_json::json!({
                    "repos": self.state.repos.iter().map(|repo| {
                        serde_json::json!({
                            "repo_id": repo.repo_id,
                            "name": repo.name,
                            "path": repo.path,
                            "origin_url": repo.origin_url,
                            "app_thread_count": repo.threads_for_surface(ThreadSurface::App).len(),
                        })
                    }).collect::<Vec<_>>()
                });
                let _ = reply.send(Ok(result));
            }
            AppControlCommand::ListThreads { repo_id, reply, .. } => {
                let result = (|| -> std::result::Result<Value, AppControlError> {
                    let repo = self
                        .state
                        .find_repo_by_id(&repo_id)
                        .cloned()
                        .ok_or_else(|| {
                            AppControlError::not_found(format!("repo not found: {repo_id}"))
                        })?;
                    Ok(serde_json::json!({
                        "repo_id": repo.repo_id,
                        "threads": repo.threads_for_surface(ThreadSurface::App)
                            .into_iter()
                            .map(|thread| self.app_thread_summary(&repo.repo_id, thread))
                            .collect::<Vec<_>>(),
                    }))
                })();
                let _ = reply.send(result);
            }
            AppControlCommand::CreateThread {
                repo_id,
                title,
                reply,
                ..
            } => {
                let result = async {
                    let repo = self
                        .state
                        .find_repo_by_id(&repo_id)
                        .cloned()
                        .ok_or_else(|| {
                            AppControlError::not_found(format!("repo not found: {repo_id}"))
                        })?;
                    let model = self.config.codex.model.clone();
                    let response = self
                        .ensure_runtime_for_repo(&repo.repo_id, None, ThreadSurface::App)
                        .await
                        .map_err(|err| AppControlError::internal(err.to_string()))?
                        .create_thread(model)
                        .await
                        .map_err(|err| AppControlError::internal(err.to_string()))?;
                    let thread = self
                        .state
                        .create_thread_for_repo(
                            &repo.repo_id,
                            response.thread.id,
                            response.thread.path,
                            title.unwrap_or_else(|| {
                                format!("Thread {}", Utc::now().format("%Y-%m-%d %H:%M"))
                            }),
                            false,
                            ThreadSurface::App,
                        )
                        .map_err(|err| AppControlError::internal(err.to_string()))?;
                    self.persist_state()
                        .map_err(|err| AppControlError::internal(err.to_string()))?;
                    Ok(self.app_thread_summary(&repo.repo_id, &thread))
                }
                .await;
                let _ = reply.send(result);
            }
            AppControlCommand::SendToThread {
                device_id,
                repo_id,
                thread_id,
                text,
                reply,
            } => {
                let result = self
                    .handle_app_send(&device_id, &repo_id, &thread_id, text)
                    .await;
                let _ = reply.send(result);
            }
            AppControlCommand::AbortRun {
                device_id,
                repo_id,
                turn_id,
                reply,
            } => {
                let result = async {
                    let run = self
                        .active_runs
                        .get(&repo_id)
                        .cloned()
                        .ok_or_else(|| AppControlError::not_found("run not found"))?;
                    match &run.route {
                        RunRoute::App { device_id: owner } if owner == &device_id => {}
                        _ => {
                            return Err(AppControlError::conflict("run belongs to another device"));
                        }
                    }
                    if run.turn_id != turn_id {
                        return Err(AppControlError::conflict(
                            "turn_id does not match the active run",
                        ));
                    }
                    let thread = self
                        .state
                        .find_thread_for_surface(&repo_id, &run.thread_local_id, ThreadSurface::App)
                        .cloned()
                        .ok_or_else(|| AppControlError::not_found("thread not found"))?;
                    self.ensure_runtime_for_repo(&repo_id, None, ThreadSurface::App)
                        .await
                        .map_err(|err| AppControlError::internal(err.to_string()))?
                        .interrupt_turn(thread.codex_thread_id, run.turn_id)
                        .await
                        .map_err(|err| AppControlError::internal(err.to_string()))?;
                    Ok(serde_json::json!({"status": "interrupt_requested"}))
                }
                .await;
                let _ = reply.send(result);
            }
            AppControlCommand::RespondApproval {
                device_id,
                repo_id,
                request_id,
                decision,
                reply,
            } => {
                let result = self
                    .handle_app_approval_response(&device_id, &repo_id, &request_id, decision)
                    .await;
                let _ = reply.send(result);
            }
        }
    }

    async fn handle_app_send(
        &mut self,
        device_id: &str,
        repo_id: &str,
        thread_id: &str,
        text: String,
    ) -> std::result::Result<Value, AppControlError> {
        if self.active_runs.contains_key(repo_id) {
            return Err(AppControlError::conflict(
                "this repo already has a running turn",
            ));
        }
        let mut thread = self
            .state
            .find_thread_for_surface(repo_id, thread_id, ThreadSurface::App)
            .cloned()
            .ok_or_else(|| AppControlError::not_found(format!("thread not found: {thread_id}")))?;
        if !thread.has_user_message {
            let title = title_from_text(&text);
            self.state
                .update_thread_title(
                    repo_id,
                    &thread.local_thread_id,
                    ThreadSurface::App,
                    title.clone(),
                )
                .map_err(|err| AppControlError::internal(err.to_string()))?;
            self.persist_state()
                .map_err(|err| AppControlError::internal(err.to_string()))?;
            thread.title = title;
            thread.has_user_message = true;
        }
        let model = self.config.codex.model.clone();
        let network_access = self.config.codex.network_access;
        let response = self
            .ensure_runtime_for_repo(repo_id, Some(&thread), ThreadSurface::App)
            .await
            .map_err(|err| AppControlError::internal(err.to_string()))?
            .start_turn(thread.codex_thread_id.clone(), text, model, network_access)
            .await
            .map_err(|err| AppControlError::internal(err.to_string()))?;
        self.active_file_changes.remove(repo_id);
        self.active_runs.insert(
            repo_id.to_string(),
            ActiveRun {
                surface: ThreadSurface::App,
                thread_local_id: thread.local_thread_id.clone(),
                turn_id: response.turn.id.clone(),
                assistant_text: String::new(),
                command_output_tail: String::new(),
                diff_preview: String::new(),
                route: RunRoute::App {
                    device_id: device_id.to_string(),
                },
                pending_request: None,
            },
        );
        Ok(serde_json::json!({
            "repo_id": repo_id,
            "thread_id": thread.local_thread_id,
            "turn_id": response.turn.id,
        }))
    }

    async fn handle_app_approval_response(
        &mut self,
        device_id: &str,
        repo_id: &str,
        request_id: &RpcId,
        decision: AppApprovalDecision,
    ) -> std::result::Result<Value, AppControlError> {
        let run = self
            .active_runs
            .get(repo_id)
            .cloned()
            .ok_or_else(|| AppControlError::not_found("run not found"))?;
        match &run.route {
            RunRoute::App { device_id: owner } if owner == device_id => {}
            _ => return Err(AppControlError::conflict("run belongs to another device")),
        }
        let pending = run
            .pending_request
            .clone()
            .ok_or_else(|| AppControlError::not_found("approval not found"))?;
        if pending_request_id(&pending) != request_id {
            return Err(AppControlError::conflict(
                "request_id does not match active approval",
            ));
        }
        let runtime = self
            .ensure_runtime_for_repo(repo_id, None, ThreadSurface::App)
            .await
            .map_err(|err| AppControlError::internal(err.to_string()))?;
        let status = match pending {
            RuntimePendingRequest::Command(pending) => {
                let decision = match decision {
                    AppApprovalDecision::Accept => CommandExecutionApprovalDecision::Accept,
                    AppApprovalDecision::Decline => CommandExecutionApprovalDecision::Decline,
                    AppApprovalDecision::Cancel => CommandExecutionApprovalDecision::Cancel,
                };
                runtime
                    .respond_command_approval(pending.request_id, decision.clone())
                    .await
                    .map_err(|err| AppControlError::internal(err.to_string()))?;
                match decision {
                    CommandExecutionApprovalDecision::Accept => "approved",
                    CommandExecutionApprovalDecision::Decline => "declined",
                    CommandExecutionApprovalDecision::Cancel => "abort requested",
                }
            }
            RuntimePendingRequest::File(pending) => {
                let decision = match decision {
                    AppApprovalDecision::Accept => FileChangeApprovalDecision::Accept,
                    AppApprovalDecision::Decline => FileChangeApprovalDecision::Decline,
                    AppApprovalDecision::Cancel => FileChangeApprovalDecision::Cancel,
                };
                runtime
                    .respond_file_approval(pending.request_id, decision.clone())
                    .await
                    .map_err(|err| AppControlError::internal(err.to_string()))?;
                match decision {
                    FileChangeApprovalDecision::Accept => "approved",
                    FileChangeApprovalDecision::Decline => "declined",
                    FileChangeApprovalDecision::Cancel => "abort requested",
                }
            }
        };
        self.cleanup_pending_request(repo_id, Some(status))
            .await
            .map_err(|err| AppControlError::internal(err.to_string()))?;
        Ok(serde_json::json!({ "status": status }))
    }

    fn app_thread_summary(&self, repo_id: &str, thread: &ThreadRecord) -> Value {
        let active_run = self
            .active_runs
            .get(repo_id)
            .filter(|run| {
                run.thread_local_id == thread.local_thread_id && run.surface == ThreadSurface::App
            })
            .map(|run| {
                serde_json::json!({
                    "turn_id": run.turn_id,
                    "assistant_text": run.assistant_text,
                    "command_output_tail": run.command_output_tail,
                    "diff_preview": run.diff_preview,
                    "pending_request": run.pending_request.as_ref().map(app_pending_summary),
                })
            });
        serde_json::json!({
            "local_thread_id": thread.local_thread_id,
            "codex_thread_id": thread.codex_thread_id,
            "title": thread.title,
            "status": format!("{:?}", thread.status).to_lowercase(),
            "created_at": thread.created_at,
            "last_used_at": thread.last_used_at,
            "has_user_message": thread.has_user_message,
            "active_run": active_run,
        })
    }

    fn find_pending_request(&self, request_id: &RpcId) -> Option<(String, RuntimePendingRequest)> {
        self.active_runs.iter().find_map(|(repo_id, run)| {
            run.pending_request
                .as_ref()
                .filter(|pending| pending_request_id(pending) == request_id)
                .cloned()
                .map(|pending| (repo_id.clone(), pending))
        })
    }

    async fn emit_app_event(&self, device_id: &str, method: &str, params: Value) {
        if let Some(gateway) = &self.app_gateway {
            if let Err(err) = gateway.send_event(device_id, method, &params).await {
                warn!(
                    "failed to push APP event {} to {}: {err}",
                    method, device_id
                );
            }
        }
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum MenuAction {
    RepoRoot,
    RepoList,
    RepoUseMenu,
    RepoUse { repo_id: String },
    RepoCloneHelp,
    RepoStatus,
    RepoRescan,
    ThreadRoot,
    ThreadList,
    ThreadNew,
    ThreadUseMenu,
    ThreadUse { thread_id: String },
    ThreadStatus,
    ApprovalRoot,
    ApprovalList,
    ApprovalRemoveMenu,
    ApprovalRemove { rule_id: String },
    ApprovalClear,
}

fn repo_menu_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup {
        inline_keyboard: vec![
            vec![
                menu_button("list", "menu:repo:list"),
                menu_button("use", "menu:repo:use"),
            ],
            vec![
                menu_button("clone", "menu:repo:clone"),
                menu_button("status", "menu:repo:status"),
            ],
            vec![menu_button("rescan", "menu:repo:rescan")],
        ],
    }
}

fn repo_use_keyboard(repos: &[crate::state::RepoRecord]) -> InlineKeyboardMarkup {
    let mut rows = pack_menu_buttons(
        repos
            .iter()
            .map(|repo| {
                menu_button(
                    &menu_label(&repo.name, &repo.repo_id),
                    &format!("menu:repo:use:{}", repo.repo_id),
                )
            })
            .collect(),
        2,
    );
    rows.push(vec![menu_button("back", "menu:repo")]);
    InlineKeyboardMarkup {
        inline_keyboard: rows,
    }
}

fn thread_menu_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup {
        inline_keyboard: vec![
            vec![
                menu_button("list", "menu:thread:list"),
                menu_button("new", "menu:thread:new"),
            ],
            vec![
                menu_button("use", "menu:thread:use"),
                menu_button("status", "menu:thread:status"),
            ],
        ],
    }
}

fn thread_use_keyboard(repo: &crate::state::RepoRecord) -> InlineKeyboardMarkup {
    let mut rows = pack_menu_buttons(
        repo.threads
            .iter()
            .map(|thread| {
                menu_button(
                    &menu_label(&thread.title, &thread.local_thread_id),
                    &format!("menu:thread:use:{}", thread.local_thread_id),
                )
            })
            .collect(),
        1,
    );
    rows.push(vec![menu_button("back", "menu:thread")]);
    InlineKeyboardMarkup {
        inline_keyboard: rows,
    }
}

fn approval_menu_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup {
        inline_keyboard: vec![
            vec![
                menu_button("list", "menu:approval:list"),
                menu_button("remove", "menu:approval:remove"),
            ],
            vec![menu_button("clear", "menu:approval:clear")],
        ],
    }
}

fn approval_remove_keyboard(rules: &[&crate::state::ApprovalRule]) -> InlineKeyboardMarkup {
    let mut rows = pack_menu_buttons(
        rules
            .iter()
            .map(|rule| {
                menu_button(
                    &menu_label(&rule.command, &rule.rule_id),
                    &format!("menu:approval:remove:{}", rule.rule_id),
                )
            })
            .collect(),
        1,
    );
    rows.push(vec![menu_button("back", "menu:approval")]);
    InlineKeyboardMarkup {
        inline_keyboard: rows,
    }
}

fn back_keyboard(action: &str) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup {
        inline_keyboard: vec![vec![menu_button("back", action)]],
    }
}

fn menu_button(text: &str, callback_data: &str) -> InlineKeyboardButton {
    InlineKeyboardButton {
        text: text.to_string(),
        callback_data: callback_data.to_string(),
    }
}

fn menu_label(label: &str, id: &str) -> String {
    let trimmed = if label.chars().count() <= 24 {
        label.to_string()
    } else {
        let shortened = label.chars().take(21).collect::<String>();
        format!("{shortened}...")
    };
    format!("{trimmed} [{}]", short_id(id))
}

fn pack_menu_buttons(
    buttons: Vec<InlineKeyboardButton>,
    per_row: usize,
) -> Vec<Vec<InlineKeyboardButton>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    for button in buttons {
        row.push(button);
        if row.len() == per_row {
            rows.push(row);
            row = Vec::new();
        }
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows
}

fn parse_menu_action(data: &str) -> Option<MenuAction> {
    let parts = data.split(':').collect::<Vec<_>>();
    match parts.as_slice() {
        ["menu", "repo"] => Some(MenuAction::RepoRoot),
        ["menu", "repo", "list"] => Some(MenuAction::RepoList),
        ["menu", "repo", "use"] => Some(MenuAction::RepoUseMenu),
        ["menu", "repo", "use", repo_id] => Some(MenuAction::RepoUse {
            repo_id: (*repo_id).to_string(),
        }),
        ["menu", "repo", "clone"] => Some(MenuAction::RepoCloneHelp),
        ["menu", "repo", "status"] => Some(MenuAction::RepoStatus),
        ["menu", "repo", "rescan"] => Some(MenuAction::RepoRescan),
        ["menu", "thread"] => Some(MenuAction::ThreadRoot),
        ["menu", "thread", "list"] => Some(MenuAction::ThreadList),
        ["menu", "thread", "new"] => Some(MenuAction::ThreadNew),
        ["menu", "thread", "use"] => Some(MenuAction::ThreadUseMenu),
        ["menu", "thread", "use", thread_id] => Some(MenuAction::ThreadUse {
            thread_id: (*thread_id).to_string(),
        }),
        ["menu", "thread", "status"] => Some(MenuAction::ThreadStatus),
        ["menu", "approval"] => Some(MenuAction::ApprovalRoot),
        ["menu", "approval", "list"] => Some(MenuAction::ApprovalList),
        ["menu", "approval", "remove"] => Some(MenuAction::ApprovalRemoveMenu),
        ["menu", "approval", "remove", rule_id] => Some(MenuAction::ApprovalRemove {
            rule_id: (*rule_id).to_string(),
        }),
        ["menu", "approval", "clear"] => Some(MenuAction::ApprovalClear),
        _ => None,
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

fn pending_request_id(value: &RuntimePendingRequest) -> &RpcId {
    match value {
        RuntimePendingRequest::Command(pending) => &pending.request_id,
        RuntimePendingRequest::File(pending) => &pending.request_id,
    }
}

fn pending_request_message_meta(value: &RuntimePendingRequest) -> Option<(i64, i64, &str)> {
    match value {
        RuntimePendingRequest::Command(pending) => pending
            .telegram_message
            .as_ref()
            .map(|message| (message.chat_id, message.message_id, message.text.as_str())),
        RuntimePendingRequest::File(pending) => pending
            .telegram_message
            .as_ref()
            .map(|message| (message.chat_id, message.message_id, message.text.as_str())),
    }
}

fn pending_request_summary(value: &RuntimePendingRequest) -> String {
    match value {
        RuntimePendingRequest::Command(pending) => format!(
            "command approval ({})",
            pending.command.clone().unwrap_or_default()
        ),
        RuntimePendingRequest::File(pending) => {
            format!("file approval ({})", pending.paths.join(", "))
        }
    }
}

fn app_pending_summary(value: &RuntimePendingRequest) -> Value {
    match value {
        RuntimePendingRequest::Command(pending) => serde_json::json!({
            "request_id": pending.request_id,
            "kind": "command",
            "thread_title": pending.thread_title,
            "turn_id": pending.turn_id,
            "item_id": pending.item_id,
            "command": pending.command,
            "cwd": pending.cwd,
            "reason": pending.reason,
        }),
        RuntimePendingRequest::File(pending) => serde_json::json!({
            "request_id": pending.request_id,
            "kind": "file",
            "thread_title": pending.thread_title,
            "turn_id": pending.turn_id,
            "item_id": pending.item_id,
            "paths": pending.paths,
            "reason": pending.reason,
            "diff_preview": pending.diff_preview,
            "preferred_decision": format!("{:?}", pending.preferred_decision).to_lowercase(),
        }),
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
    use crate::state::{ThreadStatusRecord, ThreadSurface};
    use tempfile::tempdir;

    #[test]
    fn thread_resume_path_prefers_stored_path() {
        let thread = ThreadRecord {
            local_thread_id: "local-1".into(),
            codex_thread_id: "codex-1".into(),
            codex_thread_path: Some(PathBuf::from("/tmp/thread.jsonl")),
            repo_id: "repo-1".into(),
            title: "demo".into(),
            surface: ThreadSurface::Telegram,
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

    #[test]
    fn parses_repo_menu_callback_action() {
        assert_eq!(
            parse_menu_action("menu:repo:use"),
            Some(MenuAction::RepoUseMenu)
        );
        assert_eq!(
            parse_menu_action("menu:repo:use:repo-123"),
            Some(MenuAction::RepoUse {
                repo_id: "repo-123".into(),
            })
        );
    }

    #[test]
    fn parses_approval_remove_menu_callback_action() {
        assert_eq!(
            parse_menu_action("menu:approval:remove:rule-123"),
            Some(MenuAction::ApprovalRemove {
                rule_id: "rule-123".into(),
            })
        );
    }
}
