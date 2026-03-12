#![cfg(not(any(target_os = "android", target_os = "ios")))]

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mycodex::app_auth::AppAuthStore;
use mycodex::config::{
    AppConfig, CodexConfig, Config, GitConfig, StateConfig, TelegramAccessMode, TelegramConfig,
    UiConfig, WorkspaceConfig,
};
use serde::{Deserialize, Serialize};
use tauri::Manager;

const DEFAULT_PORT: u16 = 3940;
const MAX_LOG_ENTRIES: usize = 200;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostRuntimeStatus {
    Stopped,
    Starting,
    Running,
    Stopping,
    Crashed,
}

impl Default for HostRuntimeStatus {
    fn default() -> Self {
        Self::Stopped
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostNetworkMode {
    LocalOnly,
    Lan,
}

impl Default for HostNetworkMode {
    fn default() -> Self {
        Self::LocalOnly
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostLogEntry {
    pub id: String,
    pub level: String,
    pub source: String,
    pub message: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostConfigSnapshot {
    pub network_mode: HostNetworkMode,
    pub port: u16,
    pub bind_address: String,
    pub lan_url: Option<String>,
    pub workspace_root: String,
    pub state_dir: String,
    pub config_path: String,
    pub log_path: String,
    pub codex_bin: String,
    pub binary_path: Option<String>,
    pub working_directory: Option<String>,
    pub telegram_enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStatusSnapshot {
    pub status: HostRuntimeStatus,
    pub pid: Option<u32>,
    pub last_error: Option<String>,
    pub config: HostConfigSnapshot,
    pub recent_logs: Vec<HostLogEntry>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalHostConnection {
    pub server_url: String,
    pub bearer_token: String,
    pub device_label: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostConfigUpdate {
    pub network_mode: Option<HostNetworkMode>,
    pub port: Option<u16>,
    pub workspace_root: Option<String>,
    pub state_dir: Option<String>,
    pub codex_bin: Option<String>,
    pub telegram_bot_token: Option<String>,
    pub binary_path: Option<String>,
    pub working_directory: Option<String>,
}

#[derive(Default)]
pub struct HostControllerState {
    inner: Arc<Mutex<HostController>>,
}

impl HostControllerState {
    fn lock(&self) -> Result<MutexGuard<'_, HostController>, String> {
        self.inner
            .lock()
            .map_err(|_| "host controller state is poisoned".to_string())
    }

    fn shared(&self) -> Arc<Mutex<HostController>> {
        Arc::clone(&self.inner)
    }
}

#[derive(Debug, Clone)]
struct ManagedHostConfig {
    base_dir: PathBuf,
    network_mode: HostNetworkMode,
    port: u16,
    workspace_root: PathBuf,
    state_dir: PathBuf,
    config_path: PathBuf,
    log_path: PathBuf,
    codex_bin: String,
    telegram_bot_token: String,
    binary_path: Option<PathBuf>,
    working_directory: Option<PathBuf>,
}

#[derive(Default)]
struct HostController {
    child: Option<Child>,
    status: HostRuntimeStatus,
    pid: Option<u32>,
    last_error: Option<String>,
    recent_logs: VecDeque<HostLogEntry>,
    config: Option<ManagedHostConfig>,
}

impl Drop for HostController {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl HostController {
    fn snapshot(&self) -> HostStatusSnapshot {
        HostStatusSnapshot {
            status: self.status,
            pid: self.pid,
            last_error: self.last_error.clone(),
            config: self
                .config
                .as_ref()
                .map(ManagedHostConfig::snapshot)
                .unwrap_or_else(empty_config_snapshot),
            recent_logs: self.recent_logs.iter().cloned().collect(),
        }
    }

    fn ensure_config(&mut self, app: &tauri::AppHandle) -> Result<ManagedHostConfig, String> {
        if let Some(config) = self.config.clone() {
            return Ok(config);
        }

        let app_data_dir = app
            .path()
            .app_data_dir()
            .map_err(|error| format!("failed to resolve app data directory: {error}"))?;
        let base_dir = app_data_dir.join("host");
        let config = ManagedHostConfig {
            base_dir: base_dir.clone(),
            network_mode: HostNetworkMode::LocalOnly,
            port: DEFAULT_PORT,
            workspace_root: base_dir.join("workspace"),
            state_dir: base_dir.join("state"),
            config_path: base_dir.join("config.toml"),
            log_path: base_dir.join("logs").join("host.log"),
            codex_bin: "codex".to_string(),
            telegram_bot_token: String::new(),
            binary_path: None,
            working_directory: None,
        };

        prepare_host_layout(&config)?;
        write_managed_config(&config)?;
        self.config = Some(config.clone());
        Ok(config)
    }

    fn configure(
        &mut self,
        app: &tauri::AppHandle,
        update: Option<HostConfigUpdate>,
    ) -> Result<ManagedHostConfig, String> {
        let mut config = self.ensure_config(app)?;
        if let Some(update) = update {
            apply_config_update(&mut config, update)?;
        }

        prepare_host_layout(&config)?;
        write_managed_config(&config)?;
        self.config = Some(config.clone());
        Ok(config)
    }

    fn refresh_process_state(&mut self) {
        let exit_result = match self.child.as_mut() {
            Some(child) => child.try_wait(),
            None => return,
        };

        match exit_result {
            Ok(Some(status)) => {
                self.child = None;
                self.pid = None;

                if matches!(self.status, HostRuntimeStatus::Stopping) || status.success() {
                    self.status = HostRuntimeStatus::Stopped;
                    self.push_log(
                        "info",
                        "host",
                        format!("Host process exited cleanly with status {status}."),
                    );
                    self.last_error = None;
                } else {
                    let message = format!("Host process exited unexpectedly with status {status}.");
                    self.status = HostRuntimeStatus::Crashed;
                    self.last_error = Some(message.clone());
                    self.push_log("error", "host", message);
                }
            }
            Ok(None) => {
                if matches!(self.status, HostRuntimeStatus::Starting) {
                    self.status = HostRuntimeStatus::Running;
                }
            }
            Err(error) => {
                let message = format!("Failed to inspect host process state: {error}");
                self.child = None;
                self.pid = None;
                self.status = HostRuntimeStatus::Crashed;
                self.last_error = Some(message.clone());
                self.push_log("error", "host", message);
            }
        }
    }

    fn start(
        &mut self,
        app: &tauri::AppHandle,
        update: Option<HostConfigUpdate>,
        shared: Arc<Mutex<HostController>>,
    ) -> Result<HostStatusSnapshot, String> {
        self.refresh_process_state();
        if self.child.is_some() {
            return Err("host is already running".to_string());
        }

        let config = self.configure(app, update)?;
        if is_port_in_use(config.port) {
            let message = format!(
                "APP gateway port {} is already in use. Stop the existing process or pick another port.",
                config.port
            );
            self.status = HostRuntimeStatus::Crashed;
            self.last_error = Some(message.clone());
            self.push_log("error", "host", message.clone());
            return Err(message);
        }
        self.status = HostRuntimeStatus::Starting;
        self.last_error = None;

        let mut command = build_launch_command(&config)?;
        command.stdin(Stdio::null());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());

        let mut child = command
            .spawn()
            .map_err(|error| format!("failed to start host process: {error}"))?;

        let pid = child.id();
        self.pid = Some(pid);

        if let Some(stdout) = child.stdout.take() {
            spawn_log_pump(shared.clone(), "stdout", stdout);
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_log_pump(shared, "stderr", stderr);
        }

        self.push_log(
            "info",
            "host",
            format!(
                "Started host process with pid {pid} using {}.",
                config.config_path.display()
            ),
        );
        if let Err(message) = wait_for_host_ready(&mut child, config.port) {
            let _ = child.kill();
            let _ = child.wait();
            self.pid = None;
            self.status = HostRuntimeStatus::Crashed;
            self.last_error = Some(message.clone());
            self.push_log("error", "host", message.clone());
            return Err(message);
        }
        self.child = Some(child);
        self.status = HostRuntimeStatus::Running;
        Ok(self.snapshot())
    }

    fn stop(&mut self) -> Result<HostStatusSnapshot, String> {
        self.refresh_process_state();

        let Some(mut child) = self.child.take() else {
            self.status = HostRuntimeStatus::Stopped;
            self.pid = None;
            return Ok(self.snapshot());
        };

        self.status = HostRuntimeStatus::Stopping;
        child
            .kill()
            .map_err(|error| format!("failed to stop host process: {error}"))?;
        let status = child
            .wait()
            .map_err(|error| format!("failed to wait for host process shutdown: {error}"))?;

        self.pid = None;
        self.status = HostRuntimeStatus::Stopped;
        self.last_error = None;
        self.push_log(
            "info",
            "host",
            format!("Stopped host process with status {status}."),
        );

        Ok(self.snapshot())
    }

    fn restart(
        &mut self,
        app: &tauri::AppHandle,
        update: Option<HostConfigUpdate>,
        shared: Arc<Mutex<HostController>>,
    ) -> Result<HostStatusSnapshot, String> {
        self.stop()?;
        self.start(app, update, shared)
    }

    fn push_log(&mut self, level: &str, source: &str, message: String) {
        let entry = HostLogEntry {
            id: format!("{}-{}", timestamp_string(), self.recent_logs.len() + 1),
            level: level.to_string(),
            source: source.to_string(),
            message: message.clone(),
            created_at: timestamp_string(),
        };
        self.recent_logs.push_front(entry);
        if self.recent_logs.len() > MAX_LOG_ENTRIES {
            self.recent_logs.truncate(MAX_LOG_ENTRIES);
        }

        if let Some(config) = self.config.as_ref() {
            let _ = append_log_to_file(&config.log_path, level, source, &message);
        }
    }
}

impl ManagedHostConfig {
    fn snapshot(&self) -> HostConfigSnapshot {
        HostConfigSnapshot {
            network_mode: self.network_mode,
            port: self.port,
            bind_address: self.bind_address(),
            lan_url: self.lan_url(),
            workspace_root: self.workspace_root.display().to_string(),
            state_dir: self.state_dir.display().to_string(),
            config_path: self.config_path.display().to_string(),
            log_path: self.log_path.display().to_string(),
            codex_bin: self.codex_bin.clone(),
            binary_path: self
                .binary_path
                .as_ref()
                .map(|path| path.display().to_string()),
            working_directory: self
                .working_directory
                .as_ref()
                .map(|path| path.display().to_string()),
            telegram_enabled: !self.telegram_bot_token.trim().is_empty(),
        }
    }

    fn bind_address(&self) -> String {
        match self.network_mode {
            HostNetworkMode::LocalOnly => format!("127.0.0.1:{}", self.port),
            HostNetworkMode::Lan => format!("0.0.0.0:{}", self.port),
        }
    }

    fn lan_url(&self) -> Option<String> {
        if !matches!(self.network_mode, HostNetworkMode::Lan) {
            return None;
        }
        detect_lan_ip().map(|ip| format!("http://{ip}:{}", self.port))
    }

    fn local_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

fn empty_config_snapshot() -> HostConfigSnapshot {
    HostConfigSnapshot {
        network_mode: HostNetworkMode::LocalOnly,
        port: DEFAULT_PORT,
        bind_address: format!("127.0.0.1:{DEFAULT_PORT}"),
        lan_url: None,
        workspace_root: String::new(),
        state_dir: String::new(),
        config_path: String::new(),
        log_path: String::new(),
        codex_bin: "codex".to_string(),
        binary_path: None,
        working_directory: None,
        telegram_enabled: false,
    }
}

fn apply_config_update(
    config: &mut ManagedHostConfig,
    update: HostConfigUpdate,
) -> Result<(), String> {
    if let Some(network_mode) = update.network_mode {
        config.network_mode = network_mode;
    }
    if let Some(port) = update.port {
        if port == 0 {
            return Err("port must be greater than 0".to_string());
        }
        config.port = port;
    }
    if let Some(workspace_root) = update.workspace_root {
        let trimmed = workspace_root.trim();
        if !trimmed.is_empty() {
            config.workspace_root = resolve_path(&config.base_dir, trimmed);
        }
    }
    if let Some(state_dir) = update.state_dir {
        let trimmed = state_dir.trim();
        if !trimmed.is_empty() {
            config.state_dir = resolve_path(&config.base_dir, trimmed);
        }
    }
    if let Some(codex_bin) = update.codex_bin {
        let trimmed = codex_bin.trim();
        if !trimmed.is_empty() {
            config.codex_bin = trimmed.to_string();
        }
    }
    if let Some(telegram_bot_token) = update.telegram_bot_token {
        config.telegram_bot_token = telegram_bot_token.trim().to_string();
    }
    if let Some(binary_path) = update.binary_path {
        config.binary_path = if binary_path.trim().is_empty() {
            None
        } else {
            Some(resolve_path(&config.base_dir, binary_path.trim()))
        };
    }
    if let Some(working_directory) = update.working_directory {
        config.working_directory = if working_directory.trim().is_empty() {
            None
        } else {
            Some(resolve_path(&config.base_dir, working_directory.trim()))
        };
    }
    Ok(())
}

fn resolve_path(base_dir: &Path, raw: &str) -> PathBuf {
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        path
    } else {
        base_dir.join(path)
    }
}

fn prepare_host_layout(config: &ManagedHostConfig) -> Result<(), String> {
    std::fs::create_dir_all(&config.base_dir)
        .map_err(|error| format!("failed to create host base dir: {error}"))?;
    std::fs::create_dir_all(&config.workspace_root)
        .map_err(|error| format!("failed to create workspace root: {error}"))?;
    std::fs::create_dir_all(&config.state_dir)
        .map_err(|error| format!("failed to create state dir: {error}"))?;
    if let Some(parent) = config.config_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create config dir: {error}"))?;
    }
    if let Some(parent) = config.log_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create log dir: {error}"))?;
    }
    Ok(())
}

fn write_managed_config(config: &ManagedHostConfig) -> Result<(), String> {
    let public_base_url = match config.network_mode {
        HostNetworkMode::LocalOnly => format!("http://127.0.0.1:{}", config.port),
        HostNetworkMode::Lan => config.lan_url().unwrap_or_default(),
    };

    let server_config = Config {
        workspace: WorkspaceConfig {
            root: config.workspace_root.clone(),
        },
        telegram: TelegramConfig {
            bot_token: config.telegram_bot_token.clone(),
            access_mode: TelegramAccessMode::Pairing,
            allowed_user_id: None,
            allowed_chat_id: None,
            poll_timeout_seconds: 30,
        },
        app: AppConfig {
            enabled: true,
            bind_addr: config.bind_address(),
            public_base_url,
            pairing_code_ttl_sec: 600,
        },
        codex: CodexConfig {
            bin: config.codex_bin.clone(),
            model: None,
            network_access: true,
        },
        state: StateConfig {
            dir: config.state_dir.clone(),
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
    };

    let raw = toml::to_string_pretty(&server_config)
        .map_err(|error| format!("failed to serialize host config: {error}"))?;
    std::fs::write(&config.config_path, raw)
        .map_err(|error| format!("failed to write host config: {error}"))?;
    Ok(())
}

fn build_launch_command(config: &ManagedHostConfig) -> Result<Command, String> {
    if let Some(binary_path) = config.binary_path.as_ref() {
        let mut command = Command::new(binary_path);
        command
            .arg("serve")
            .arg("--config")
            .arg(&config.config_path);
        if let Some(working_directory) = config.working_directory.as_ref() {
            command.current_dir(working_directory);
        }
        return Ok(command);
    }

    let current_exe = std::env::current_exe()
        .map_err(|error| format!("failed to resolve current executable: {error}"))?;
    let mut command = Command::new(current_exe);
    command
        .arg("--mycodex-host-run")
        .arg("serve")
        .arg("--config")
        .arg(&config.config_path);
    if let Some(working_directory) = config.working_directory.as_ref() {
        command.current_dir(working_directory);
    }
    Ok(command)
}

fn is_port_in_use(port: u16) -> bool {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&address, Duration::from_millis(150)).is_ok()
}

fn wait_for_host_ready(child: &mut Child, port: u16) -> Result<(), String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));

    while std::time::Instant::now() < deadline {
        if TcpStream::connect_timeout(&address, Duration::from_millis(150)).is_ok() {
            return Ok(());
        }

        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("failed to inspect host startup: {error}"))?
        {
            return Err(format!("Host exited during startup with status {status}."));
        }

        thread::sleep(Duration::from_millis(100));
    }

    Err(format!(
        "Host process started, but APP gateway did not become reachable on 127.0.0.1:{port}."
    ))
}

fn detect_lan_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    Some(socket.local_addr().ok()?.ip())
}

fn spawn_log_pump<R>(shared: Arc<Mutex<HostController>>, source: &'static str, reader: R)
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let buffered = BufReader::new(reader);
        for line in buffered.lines() {
            match line {
                Ok(line) => append_log(&shared, "info", source, line),
                Err(error) => {
                    append_log(
                        &shared,
                        "error",
                        source,
                        format!("failed to read host output: {error}"),
                    );
                    break;
                }
            }
        }
    });
}

fn append_log(shared: &Arc<Mutex<HostController>>, level: &str, source: &str, message: String) {
    if let Ok(mut controller) = shared.lock() {
        controller.push_log(level, source, message);
    }
}

fn append_log_to_file(
    log_path: &Path,
    level: &str,
    source: &str,
    message: &str,
) -> Result<(), String> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .map_err(|error| format!("failed to open host log file: {error}"))?;
    writeln!(
        file,
        "[{}] [{}] [{}] {}",
        timestamp_string(),
        level,
        source,
        message
    )
    .map_err(|error| format!("failed to append host log file: {error}"))?;
    Ok(())
}

fn timestamp_string() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", now.as_secs())
}

#[tauri::command]
pub fn get_host_status(
    app: tauri::AppHandle,
    state: tauri::State<'_, HostControllerState>,
) -> Result<HostStatusSnapshot, String> {
    let mut controller = state.lock()?;
    controller.configure(&app, None)?;
    controller.refresh_process_state();
    Ok(controller.snapshot())
}

#[tauri::command]
pub fn update_host_config(
    app: tauri::AppHandle,
    state: tauri::State<'_, HostControllerState>,
    request: HostConfigUpdate,
) -> Result<HostStatusSnapshot, String> {
    let mut controller = state.lock()?;
    controller.configure(&app, Some(request))?;
    Ok(controller.snapshot())
}

#[tauri::command]
pub fn start_host(
    app: tauri::AppHandle,
    state: tauri::State<'_, HostControllerState>,
    request: Option<HostConfigUpdate>,
) -> Result<HostStatusSnapshot, String> {
    let shared = state.shared();
    let mut controller = state.lock()?;
    controller.start(&app, request, shared)
}

#[tauri::command]
pub fn stop_host(
    state: tauri::State<'_, HostControllerState>,
) -> Result<HostStatusSnapshot, String> {
    let mut controller = state.lock()?;
    controller.stop()
}

#[tauri::command]
pub fn restart_host(
    app: tauri::AppHandle,
    state: tauri::State<'_, HostControllerState>,
    request: Option<HostConfigUpdate>,
) -> Result<HostStatusSnapshot, String> {
    let shared = state.shared();
    let mut controller = state.lock()?;
    controller.restart(&app, request, shared)
}

#[tauri::command]
pub fn read_host_logs(
    state: tauri::State<'_, HostControllerState>,
) -> Result<Vec<HostLogEntry>, String> {
    let mut controller = state.lock()?;
    controller.refresh_process_state();
    Ok(controller.snapshot().recent_logs)
}

#[tauri::command]
pub fn issue_local_host_connection(
    app: tauri::AppHandle,
    state: tauri::State<'_, HostControllerState>,
) -> Result<LocalHostConnection, String> {
    let mut controller = state.lock()?;
    let config = controller.configure(&app, None)?;
    let auth_store = AppAuthStore::new(config.state_dir.join("app_auth.json"));
    let pairing = auth_store
        .create_pairing_request("MyCodex Desktop".to_string(), 600)
        .map_err(|error| format!("failed to create local pairing request: {error}"))?;
    auth_store
        .approve_pairing_code(&pairing.code)
        .map_err(|error| format!("failed to auto-approve local pairing request: {error}"))?;
    let poll = auth_store
        .poll_pairing(&pairing.pairing_id)
        .map_err(|error| format!("failed to fetch local pairing token: {error}"))?;
    let bearer_token = poll
        .token
        .ok_or_else(|| "local pairing did not yield a bearer token".to_string())?;

    Ok(LocalHostConnection {
        server_url: config.local_url(),
        bearer_token,
        device_label: "MyCodex Desktop".to_string(),
    })
}
