use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, mpsc, oneshot};
use tracing::{debug, info, warn};

use super::protocol::{
    AgentMessageDeltaNotification, AskForApproval, ClientInfo, CommandExecutionApprovalDecision,
    CommandExecutionOutputDeltaNotification, CommandExecutionRequestApprovalParams,
    CommandExecutionRequestApprovalResponse, ErrorNotification, FileChangeApprovalDecision,
    FileChangeRequestApprovalParams, FileChangeRequestApprovalResponse, InitializeCapabilities,
    InitializeParams, InitializeResult, InitializedParams, ItemCompletedNotification,
    ItemStartedNotification, Personality, RpcErrorResponse, RpcId, RpcNotification, RpcRequest,
    RpcSuccessResponse, SandboxMode, SandboxPolicy, ServerRequestResolvedNotification,
    ThreadResumeParams, ThreadResumeResponse, ThreadStartParams, ThreadStartResponse,
    TurnCompletedNotification, TurnDiffUpdatedNotification, TurnInputItem, TurnInterruptParams,
    TurnStartParams, TurnStartResponse, TurnStartedNotification,
};

type PendingResponseMap = Arc<Mutex<HashMap<RpcId, oneshot::Sender<Result<Value>>>>>;

#[derive(Debug, Clone)]
pub enum CodexEvent {
    RuntimeExited {
        repo_id: String,
        status_code: Option<i32>,
        error: Option<String>,
    },
    TurnStarted {
        repo_id: String,
        payload: TurnStartedNotification,
    },
    TurnCompleted {
        repo_id: String,
        payload: TurnCompletedNotification,
    },
    AgentMessageDelta {
        repo_id: String,
        payload: AgentMessageDeltaNotification,
    },
    CommandOutputDelta {
        repo_id: String,
        payload: CommandExecutionOutputDeltaNotification,
    },
    DiffUpdated {
        repo_id: String,
        payload: TurnDiffUpdatedNotification,
    },
    ItemStarted {
        repo_id: String,
        payload: ItemStartedNotification,
    },
    ItemCompleted {
        repo_id: String,
        payload: ItemCompletedNotification,
    },
    CommandApprovalRequested {
        repo_id: String,
        request_id: RpcId,
        params: CommandExecutionRequestApprovalParams,
    },
    FileApprovalRequested {
        repo_id: String,
        request_id: RpcId,
        params: FileChangeRequestApprovalParams,
    },
    ServerRequestResolved {
        repo_id: String,
        payload: ServerRequestResolvedNotification,
    },
    Error {
        repo_id: String,
        payload: ErrorNotification,
    },
}

pub struct CodexRuntime {
    repo_id: String,
    repo_path: PathBuf,
    stdin: BufWriter<ChildStdin>,
    child: Child,
    next_request_id: u64,
    pending: PendingResponseMap,
    stdout_task: tokio::task::JoinHandle<()>,
    stderr_task: tokio::task::JoinHandle<()>,
}

impl CodexRuntime {
    pub async fn start(
        codex_bin: &str,
        repo_id: String,
        repo_path: PathBuf,
        app_events: mpsc::Sender<CodexEvent>,
    ) -> Result<Self> {
        let mut command = Command::new(codex_bin);
        command
            .arg("app-server")
            .current_dir(&repo_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command
            .spawn()
            .with_context(|| format!("failed to start codex app-server via {codex_bin}"))?;

        let stdin = child
            .stdin
            .take()
            .context("codex app-server stdin unavailable")?;
        let stdout = child
            .stdout
            .take()
            .context("codex app-server stdout unavailable")?;
        let stderr = child
            .stderr
            .take()
            .context("codex app-server stderr unavailable")?;

        let pending = Arc::new(Mutex::new(
            HashMap::<RpcId, oneshot::Sender<Result<Value>>>::new(),
        ));
        let stdout_task = spawn_stdout_reader(
            repo_id.clone(),
            stdout,
            app_events.clone(),
            Arc::clone(&pending),
        );
        let stderr_task = spawn_stderr_reader(repo_id.clone(), stderr);

        let mut runtime = Self {
            repo_id: repo_id.clone(),
            repo_path,
            stdin: BufWriter::new(stdin),
            child,
            next_request_id: 1,
            pending,
            stdout_task,
            stderr_task,
        };

        runtime.initialize().await?;
        info!("started codex runtime for repo {repo_id}");
        Ok(runtime)
    }

    pub fn repo_id(&self) -> &str {
        &self.repo_id
    }

    pub fn repo_path(&self) -> &Path {
        &self.repo_path
    }

    pub fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>> {
        self.child
            .try_wait()
            .context("failed to poll codex runtime process")
    }

    pub async fn stop(mut self) -> Result<()> {
        if let Err(err) = self.child.kill().await {
            warn!(
                "failed to kill codex runtime for repo {}: {err}",
                self.repo_id
            );
        }
        let _ = self.child.wait().await;
        self.stdout_task.abort();
        self.stderr_task.abort();
        Ok(())
    }

    pub async fn create_thread(&mut self, model: Option<String>) -> Result<ThreadStartResponse> {
        let params = ThreadStartParams {
            model,
            cwd: Some(self.repo_path.display().to_string()),
            approval_policy: Some(AskForApproval::UnlessTrusted),
            sandbox: Some(SandboxMode::WorkspaceWrite),
            personality: Some(Personality::Pragmatic),
        };
        self.send_request("thread/start", params).await
    }

    pub async fn resume_thread(
        &mut self,
        thread_id: String,
        thread_path: Option<PathBuf>,
        model: Option<String>,
    ) -> Result<ThreadResumeResponse> {
        let params = ThreadResumeParams {
            thread_id,
            path: thread_path,
            model,
            cwd: Some(self.repo_path.display().to_string()),
            approval_policy: Some(AskForApproval::UnlessTrusted),
            sandbox: Some(SandboxMode::WorkspaceWrite),
            personality: Some(Personality::Pragmatic),
        };
        self.send_request("thread/resume", params).await
    }

    pub async fn start_turn(
        &mut self,
        thread_id: String,
        text: String,
        model: Option<String>,
        network_access: bool,
    ) -> Result<TurnStartResponse> {
        let params = TurnStartParams {
            thread_id,
            input: vec![TurnInputItem::Text { text }],
            cwd: Some(self.repo_path.clone()),
            approval_policy: Some(AskForApproval::UnlessTrusted),
            sandbox_policy: Some(SandboxPolicy::WorkspaceWrite {
                writable_roots: vec![self.repo_path.clone()],
                network_access,
            }),
            model,
            personality: Some(Personality::Pragmatic),
        };
        self.send_request("turn/start", params).await
    }

    pub async fn interrupt_turn(&mut self, thread_id: String, turn_id: String) -> Result<()> {
        let params = TurnInterruptParams { thread_id, turn_id };
        let _: Value = self.send_request("turn/interrupt", params).await?;
        Ok(())
    }

    pub async fn respond_command_approval(
        &mut self,
        request_id: RpcId,
        decision: CommandExecutionApprovalDecision,
    ) -> Result<()> {
        let response = CommandExecutionRequestApprovalResponse { decision };
        self.write_response(request_id, response).await
    }

    pub async fn respond_file_approval(
        &mut self,
        request_id: RpcId,
        decision: FileChangeApprovalDecision,
    ) -> Result<()> {
        let response = FileChangeRequestApprovalResponse { decision };
        self.write_response(request_id, response).await
    }

    async fn initialize(&mut self) -> Result<()> {
        let init = InitializeParams {
            client_info: ClientInfo {
                name: "mycodex".into(),
                title: "MyCodex".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            capabilities: Some(InitializeCapabilities {
                experimental_api: true,
                opt_out_notification_methods: None,
            }),
        };

        let _: InitializeResult = self.send_request("initialize", init).await?;
        let notification = RpcNotification {
            method: "initialized".to_string(),
            params: InitializedParams::default(),
        };
        self.write_message(&notification).await
    }

    async fn write_response<T: Serialize>(&mut self, id: RpcId, result: T) -> Result<()> {
        let payload = serde_json::json!({
            "id": id,
            "result": result,
        });
        self.write_raw(payload).await
    }

    async fn send_request<P: Serialize, T: DeserializeOwned>(
        &mut self,
        method: &str,
        params: P,
    ) -> Result<T> {
        let id = RpcId::Number(self.next_request_id);
        self.next_request_id += 1;

        let request = RpcRequest {
            id: id.clone(),
            method: method.to_string(),
            params,
        };

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id.clone(), tx);
        self.write_message(&request).await?;

        let value = rx
            .await
            .map_err(|_| anyhow!("codex runtime dropped response for {method}"))??;
        serde_json::from_value(value)
            .with_context(|| format!("failed to decode codex response for {method}"))
    }

    async fn write_message<T: Serialize>(&mut self, payload: &T) -> Result<()> {
        let raw = serde_json::to_vec(payload).context("failed to encode codex JSON")?;
        self.stdin
            .write_all(&raw)
            .await
            .context("failed to write codex request")?;
        self.stdin
            .write_all(b"\n")
            .await
            .context("failed to terminate codex request line")?;
        self.stdin
            .flush()
            .await
            .context("failed to flush codex stdin")
    }

    async fn write_raw(&mut self, payload: Value) -> Result<()> {
        let raw = serde_json::to_vec(&payload).context("failed to encode raw codex JSON")?;
        self.stdin
            .write_all(&raw)
            .await
            .context("failed to write codex raw request")?;
        self.stdin
            .write_all(b"\n")
            .await
            .context("failed to terminate codex raw request line")?;
        self.stdin
            .flush()
            .await
            .context("failed to flush codex stdin")
    }
}

fn spawn_stdout_reader(
    repo_id: String,
    stdout: ChildStdout,
    app_events: mpsc::Sender<CodexEvent>,
    pending: PendingResponseMap,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();

        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    if let Err(err) =
                        handle_stdout_line(repo_id.clone(), &line, &app_events, &pending).await
                    {
                        warn!("failed to process codex line for repo {}: {err}", repo_id);
                    }
                }
                Ok(None) => break,
                Err(err) => {
                    warn!("failed to read codex stdout for repo {}: {err}", repo_id);
                    break;
                }
            }
        }
    })
}

fn spawn_stderr_reader(repo_id: String, stderr: ChildStderr) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let reader = BufReader::new(stderr);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            debug!("[codex:{}] {}", repo_id, line);
        }
    })
}

async fn handle_stdout_line(
    repo_id: String,
    line: &str,
    app_events: &mpsc::Sender<CodexEvent>,
    pending: &PendingResponseMap,
) -> Result<()> {
    let value: Value = serde_json::from_str(line).context("invalid codex JSON line")?;

    if value.get("method").is_some() {
        dispatch_server_message(repo_id, value, app_events).await?;
        return Ok(());
    }

    if value.get("id").is_some() {
        let _response_id: RpcId = serde_json::from_value(
            value
                .get("id")
                .cloned()
                .context("codex response missing id")?,
        )
        .context("invalid codex response id")?;

        if value.get("error").is_some() {
            let response: RpcErrorResponse =
                serde_json::from_value(value).context("invalid codex error response")?;
            if let Some(tx) = pending.lock().await.remove(&response.id) {
                let message = format!(
                    "codex error {}: {}",
                    response.error.code, response.error.message
                );
                let _ = tx.send(Err(anyhow!(message)));
            }
            return Ok(());
        }

        let response: RpcSuccessResponse<Value> =
            serde_json::from_value(value).context("invalid codex success response")?;
        if let Some(tx) = pending.lock().await.remove(&response.id) {
            let _ = tx.send(Ok(response.result));
        } else {
            warn!("received unexpected codex response id {}", response.id);
        }
        return Ok(());
    }

    bail!("unrecognized codex message shape")
}

async fn dispatch_server_message(
    repo_id: String,
    value: Value,
    app_events: &mpsc::Sender<CodexEvent>,
) -> Result<()> {
    let method = value
        .get("method")
        .and_then(Value::as_str)
        .context("codex server message missing method")?
        .to_string();

    if value.get("id").is_some() {
        let request_id: RpcId = serde_json::from_value(
            value
                .get("id")
                .cloned()
                .context("codex request missing id")?,
        )
        .context("invalid codex request id")?;
        let params = value.get("params").cloned().unwrap_or(Value::Null);
        match method.as_str() {
            "item/commandExecution/requestApproval" => {
                let params: CommandExecutionRequestApprovalParams =
                    serde_json::from_value(params).context("invalid command approval request")?;
                app_events
                    .send(CodexEvent::CommandApprovalRequested {
                        repo_id,
                        request_id,
                        params,
                    })
                    .await
                    .ok();
            }
            "item/fileChange/requestApproval" => {
                let params: FileChangeRequestApprovalParams =
                    serde_json::from_value(params).context("invalid file approval request")?;
                app_events
                    .send(CodexEvent::FileApprovalRequested {
                        repo_id,
                        request_id,
                        params,
                    })
                    .await
                    .ok();
            }
            _ => {
                warn!("ignoring unsupported codex server request: {}", method);
            }
        }
        return Ok(());
    }

    let params = value.get("params").cloned().unwrap_or(Value::Null);
    let event = match method.as_str() {
        "turn/started" => CodexEvent::TurnStarted {
            repo_id,
            payload: serde_json::from_value(params).context("invalid turn/started notification")?,
        },
        "turn/completed" => CodexEvent::TurnCompleted {
            repo_id,
            payload: serde_json::from_value(params)
                .context("invalid turn/completed notification")?,
        },
        "item/agentMessage/delta" => CodexEvent::AgentMessageDelta {
            repo_id,
            payload: serde_json::from_value(params)
                .context("invalid item/agentMessage/delta notification")?,
        },
        "item/commandExecution/outputDelta" => CodexEvent::CommandOutputDelta {
            repo_id,
            payload: serde_json::from_value(params)
                .context("invalid item/commandExecution/outputDelta notification")?,
        },
        "turn/diff/updated" => CodexEvent::DiffUpdated {
            repo_id,
            payload: serde_json::from_value(params).context("invalid turn/diff/updated")?,
        },
        "item/started" => CodexEvent::ItemStarted {
            repo_id,
            payload: serde_json::from_value(params).context("invalid item/started")?,
        },
        "item/completed" => CodexEvent::ItemCompleted {
            repo_id,
            payload: serde_json::from_value(params).context("invalid item/completed")?,
        },
        "serverRequest/resolved" => CodexEvent::ServerRequestResolved {
            repo_id,
            payload: serde_json::from_value(params)
                .context("invalid serverRequest/resolved notification")?,
        },
        "error" => CodexEvent::Error {
            repo_id,
            payload: serde_json::from_value(params).context("invalid error notification")?,
        },
        other => {
            debug!("ignoring unsupported codex notification {}", other);
            return Ok(());
        }
    };
    app_events.send(event).await.ok();
    Ok(())
}
