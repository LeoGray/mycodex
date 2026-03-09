use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{Mutex, mpsc, oneshot};
use tower_http::cors::{Any, CorsLayer};
use tracing::{info, warn};

use crate::app_auth::{AppAuthStore, AppPairingPollResult};
use crate::codex::protocol::RpcId;
use crate::config::AppConfig;

#[derive(Debug)]
pub enum AppControlCommand {
    ListRepos {
        device_id: String,
        reply: AppControlReply,
    },
    ListThreads {
        device_id: String,
        repo_id: String,
        reply: AppControlReply,
    },
    CreateThread {
        device_id: String,
        repo_id: String,
        title: Option<String>,
        reply: AppControlReply,
    },
    SendToThread {
        device_id: String,
        repo_id: String,
        thread_id: String,
        text: String,
        reply: AppControlReply,
    },
    AbortRun {
        device_id: String,
        repo_id: String,
        turn_id: String,
        reply: AppControlReply,
    },
    RespondApproval {
        device_id: String,
        repo_id: String,
        request_id: RpcId,
        decision: AppApprovalDecision,
        reply: AppControlReply,
    },
}

pub type AppControlReply = oneshot::Sender<std::result::Result<Value, AppControlError>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppControlError {
    pub code: i64,
    pub message: String,
}

impl AppControlError {
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: -32000,
            message: message.into(),
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            code: 404,
            message: message.into(),
        }
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            code: 409,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppApprovalDecision {
    Accept,
    Decline,
    Cancel,
}

#[derive(Debug, Clone)]
pub struct AppGatewayHandle {
    state: Arc<AppGatewayState>,
}

#[derive(Debug)]
struct AppGatewayState {
    auth_store: AppAuthStore,
    control_tx: mpsc::Sender<AppControlCommand>,
    connections: Mutex<HashMap<String, mpsc::Sender<String>>>,
}

#[derive(Clone)]
struct HttpState {
    gateway: AppGatewayHandle,
    config: AppConfig,
}

#[derive(Debug, Deserialize)]
struct PairingRequestPayload {
    device_label: String,
}

#[derive(Debug, Serialize)]
struct PairingRequestResponse {
    pairing_id: String,
    pairing_code: String,
    expires_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize)]
struct WsQuery {
    token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    id: Value,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<AppControlError>,
}

#[derive(Debug, Deserialize)]
struct ThreadsListParams {
    repo_id: String,
}

#[derive(Debug, Deserialize)]
struct ThreadsCreateParams {
    repo_id: String,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ThreadsSendParams {
    repo_id: String,
    thread_id: String,
    text: String,
}

#[derive(Debug, Deserialize)]
struct RunsAbortParams {
    repo_id: String,
    turn_id: String,
}

#[derive(Debug, Deserialize)]
struct ApprovalsRespondParams {
    repo_id: String,
    request_id: RpcId,
    decision: AppApprovalDecision,
}

impl AppGatewayHandle {
    pub fn spawn(
        config: AppConfig,
        auth_store: AppAuthStore,
        control_tx: mpsc::Sender<AppControlCommand>,
    ) -> Result<Self> {
        let bind_addr: SocketAddr = config
            .bind_addr
            .parse()
            .with_context(|| format!("invalid APP bind address {}", config.bind_addr))?;
        let handle = Self {
            state: Arc::new(AppGatewayState {
                auth_store,
                control_tx,
                connections: Mutex::new(HashMap::new()),
            }),
        };

        let state = HttpState {
            gateway: handle.clone(),
            config: config.clone(),
        };
        let router = Router::new()
            .route("/healthz", get(healthz))
            .route("/api/app/pairings/request", post(request_pairing))
            .route("/api/app/pairings/{pairing_id}", get(get_pairing))
            .route("/ws", get(ws_handler))
            .layer(
                CorsLayer::new()
                    .allow_methods(Any)
                    .allow_headers(Any)
                    .allow_origin(Any),
            )
            .with_state(state);

        tokio::spawn(async move {
            match tokio::net::TcpListener::bind(bind_addr).await {
                Ok(listener) => {
                    info!("APP gateway listening on {}", bind_addr);
                    if let Err(err) = axum::serve(listener, router).await {
                        warn!("APP gateway exited with error: {err}");
                    }
                }
                Err(err) => warn!("failed to bind APP gateway {}: {err}", bind_addr),
            }
        });

        Ok(handle)
    }

    pub async fn send_event<T: Serialize>(
        &self,
        device_id: &str,
        method: &str,
        params: &T,
    ) -> Result<()> {
        let payload = serde_json::json!({
            "method": method,
            "params": params,
        });
        let message = serde_json::to_string(&payload).context("failed to encode APP event")?;
        let sender = {
            let connections = self.state.connections.lock().await;
            connections.get(device_id).cloned()
        };
        if let Some(sender) = sender {
            sender
                .send(message)
                .await
                .map_err(|_| anyhow!("APP device is not connected"))?;
        }
        Ok(())
    }

    pub async fn disconnect_device(&self, device_id: &str) {
        self.state.connections.lock().await.remove(device_id);
    }
}

async fn healthz() -> &'static str {
    "ok"
}

async fn request_pairing(
    State(state): State<HttpState>,
    Json(payload): Json<PairingRequestPayload>,
) -> std::result::Result<Json<PairingRequestResponse>, AppGatewayHttpError> {
    let device_label = payload.device_label.trim();
    if device_label.is_empty() {
        return Err(AppGatewayHttpError::bad_request(
            "device_label must not be empty",
        ));
    }
    let pairing = state
        .gateway
        .state
        .auth_store
        .create_pairing_request(device_label.to_string(), state.config.pairing_code_ttl_sec)
        .map_err(AppGatewayHttpError::internal)?;
    Ok(Json(PairingRequestResponse {
        pairing_id: pairing.pairing_id,
        pairing_code: pairing.code,
        expires_at: pairing.expires_at,
    }))
}

async fn get_pairing(
    State(state): State<HttpState>,
    Path(pairing_id): Path<String>,
) -> std::result::Result<Json<AppPairingPollResult>, AppGatewayHttpError> {
    let result = state
        .gateway
        .state
        .auth_store
        .poll_pairing(&pairing_id)
        .map_err(|err| {
            if err.to_string().contains("not found") {
                AppGatewayHttpError::not_found(err)
            } else {
                AppGatewayHttpError::internal(err)
            }
        })?;
    Ok(Json(result))
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<WsQuery>,
) -> std::result::Result<Response, AppGatewayHttpError> {
    let token = extract_bearer_token(&headers, query.token.as_deref())
        .ok_or_else(|| AppGatewayHttpError::unauthorized("missing APP bearer token"))?;
    let device = state
        .gateway
        .state
        .auth_store
        .authenticate_token(&token)
        .map_err(AppGatewayHttpError::internal)?
        .ok_or_else(|| AppGatewayHttpError::unauthorized("invalid APP bearer token"))?;
    state
        .gateway
        .state
        .auth_store
        .touch_last_seen(&device.device_id)
        .map_err(AppGatewayHttpError::internal)?;

    Ok(ws.on_upgrade(move |socket| handle_ws(socket, state.gateway, device.device_id)))
}

async fn handle_ws(socket: WebSocket, gateway: AppGatewayHandle, device_id: String) {
    let (mut writer, mut reader) = socket.split();
    let (tx, mut rx) = mpsc::channel::<String>(128);
    let old_sender = gateway
        .state
        .connections
        .lock()
        .await
        .insert(device_id.clone(), tx);
    drop(old_sender);

    loop {
        tokio::select! {
            maybe_outbound = rx.recv() => {
                let Some(message) = maybe_outbound else {
                    break;
                };
                if writer.send(Message::Text(message.into())).await.is_err() {
                    break;
                }
            }
            inbound = reader.next() => {
                match inbound {
                    Some(Ok(Message::Text(text))) => {
                        if let Err(err) = handle_ws_request(&gateway, &device_id, &mut writer, text.to_string()).await {
                            let payload = serde_json::json!({
                                "id": Value::Null,
                                "error": {
                                    "code": err.code,
                                    "message": err.message,
                                }
                            });
                            if let Ok(raw) = serde_json::to_string(&payload) {
                                let _ = writer.send(Message::Text(raw.into())).await;
                            }
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        if writer.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(err)) => {
                        warn!("APP websocket error for device {}: {err}", device_id);
                        break;
                    }
                }
            }
        }
    }

    gateway.disconnect_device(&device_id).await;
}

async fn handle_ws_request(
    gateway: &AppGatewayHandle,
    device_id: &str,
    writer: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    raw: String,
) -> std::result::Result<(), AppControlError> {
    let request: JsonRpcRequest = serde_json::from_str(&raw).map_err(|err| {
        AppControlError::invalid_params(format!("invalid JSON-RPC request: {err}"))
    })?;
    let (reply_tx, reply_rx) = oneshot::channel();
    let command = command_from_request(device_id.to_string(), &request, reply_tx)?;
    gateway
        .state
        .control_tx
        .send(command)
        .await
        .map_err(|_| AppControlError::internal("APP control plane is unavailable"))?;
    let response = match reply_rx.await {
        Ok(Ok(result)) => JsonRpcResponse {
            id: request.id.clone(),
            result: Some(result),
            error: None,
        },
        Ok(Err(error)) => JsonRpcResponse {
            id: request.id.clone(),
            result: None,
            error: Some(error),
        },
        Err(_) => {
            return Err(AppControlError::internal(
                "APP control plane dropped the response",
            ));
        }
    };
    let payload = serde_json::to_string(&response)
        .map_err(|err| AppControlError::internal(format!("failed to encode response: {err}")))?;
    writer
        .send(Message::Text(payload.into()))
        .await
        .map_err(|_| AppControlError::internal("failed to write websocket response"))?;
    Ok(())
}

fn command_from_request(
    device_id: String,
    request: &JsonRpcRequest,
    reply: AppControlReply,
) -> std::result::Result<AppControlCommand, AppControlError> {
    match request.method.as_str() {
        "repos.list" => Ok(AppControlCommand::ListRepos { device_id, reply }),
        "threads.list" => {
            let params: ThreadsListParams = serde_json::from_value(request.params.clone())
                .map_err(|err| AppControlError::invalid_params(err.to_string()))?;
            Ok(AppControlCommand::ListThreads {
                device_id,
                repo_id: params.repo_id,
                reply,
            })
        }
        "threads.create" => {
            let params: ThreadsCreateParams = serde_json::from_value(request.params.clone())
                .map_err(|err| AppControlError::invalid_params(err.to_string()))?;
            Ok(AppControlCommand::CreateThread {
                device_id,
                repo_id: params.repo_id,
                title: params.title,
                reply,
            })
        }
        "threads.send" => {
            let params: ThreadsSendParams = serde_json::from_value(request.params.clone())
                .map_err(|err| AppControlError::invalid_params(err.to_string()))?;
            Ok(AppControlCommand::SendToThread {
                device_id,
                repo_id: params.repo_id,
                thread_id: params.thread_id,
                text: params.text,
                reply,
            })
        }
        "runs.abort" => {
            let params: RunsAbortParams = serde_json::from_value(request.params.clone())
                .map_err(|err| AppControlError::invalid_params(err.to_string()))?;
            Ok(AppControlCommand::AbortRun {
                device_id,
                repo_id: params.repo_id,
                turn_id: params.turn_id,
                reply,
            })
        }
        "approvals.respond" => {
            let params: ApprovalsRespondParams = serde_json::from_value(request.params.clone())
                .map_err(|err| AppControlError::invalid_params(err.to_string()))?;
            Ok(AppControlCommand::RespondApproval {
                device_id,
                repo_id: params.repo_id,
                request_id: params.request_id,
                decision: params.decision,
                reply,
            })
        }
        _ => Err(AppControlError {
            code: -32601,
            message: format!("unknown APP method {}", request.method),
        }),
    }
}

fn extract_bearer_token(headers: &HeaderMap, query_token: Option<&str>) -> Option<String> {
    if let Some(value) = query_token {
        if !value.trim().is_empty() {
            return Some(value.to_string());
        }
    }

    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(|value| value.to_string())
}

#[derive(Debug)]
struct AppGatewayHttpError {
    status: StatusCode,
    message: String,
}

impl AppGatewayHttpError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: message.into(),
        }
    }

    fn not_found(err: anyhow::Error) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: err.to_string(),
        }
    }

    fn internal(err: anyhow::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: err.to_string(),
        }
    }
}

impl IntoResponse for AppGatewayHttpError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({
                "error": self.message,
            })),
        )
            .into_response()
    }
}
