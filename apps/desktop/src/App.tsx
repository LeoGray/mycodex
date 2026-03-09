import { useEffect, useRef, useState } from "react";

import { GatewayRpcClient, pollPairing, requestPairing } from "./lib/gateway";
import {
  selectedThreadForRepo,
  useSessionStore,
  withPendingRequest,
} from "./store/session";
import type {
  AppPendingRequest,
  AppRepoSummary,
  GatewayNotification,
  PairingPollResponse,
  RunApprovalRequiredEvent,
} from "./types/gateway";

function humanConnectionLabel(
  status: ReturnType<typeof useSessionStore.getState>["connectionStatus"],
): string {
  switch (status) {
    case "pairing":
      return "Pairing pending";
    case "connecting":
      return "Connecting";
    case "connected":
      return "Connected";
    case "error":
      return "Needs attention";
    default:
      return "Not connected";
  }
}

function formatTimestamp(value: string | null | undefined): string {
  if (!value) {
    return "n/a";
  }

  const timestamp = new Date(value);
  if (Number.isNaN(timestamp.getTime())) {
    return value;
  }

  return new Intl.DateTimeFormat(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  }).format(timestamp);
}

function shortId(value: string | null | undefined): string {
  return value ? value.slice(0, 8) : "n/a";
}

function pendingRequestFromEvent(event: RunApprovalRequiredEvent): AppPendingRequest {
  if (event.kind === "command") {
    return {
      request_id: event.request_id,
      kind: "command",
      thread_title: event.thread_title,
      turn_id: event.turn_id,
      item_id: "",
      command: event.command ?? null,
      cwd: event.cwd ?? null,
      reason: event.reason ?? null,
    };
  }

  return {
    request_id: event.request_id,
    kind: "file",
    thread_title: event.thread_title,
    turn_id: event.turn_id,
    item_id: "",
    paths: event.paths ?? [],
    reason: event.reason ?? null,
    diff_preview: event.diff_preview ?? "",
    preferred_decision: "accept",
  };
}

function describeRepo(repo: AppRepoSummary): string {
  return repo.app_thread_count === 1
    ? "1 APP thread"
    : `${repo.app_thread_count} APP threads`;
}

function describePendingRequest(pendingRequest: AppPendingRequest): string {
  if (pendingRequest.kind === "command") {
    return pendingRequest.command || "Command approval";
  }
  return pendingRequest.paths.join(", ") || "File approval";
}

function errorMessage(error: unknown, fallback: string): string {
  return error instanceof Error ? error.message : fallback;
}

export default function App() {
  const {
    serverUrl,
    deviceLabel,
    bearerToken,
    connectionStatus,
    lastError,
    pairingSession,
    repos,
    threadsByRepo,
    selectedRepoId,
    selectedThreadIdByRepo,
    runtimeLog,
    setServerUrl,
    setDeviceLabel,
    setBearerToken,
    setConnectionStatus,
    setLastError,
    setPairingSession,
    setRepos,
    setThreads,
    upsertThread,
    selectRepo,
    selectThread,
    updateThreadRun,
    clearRuntimeState,
    pushRuntimeLog,
  } = useSessionStore();

  const [messageInput, setMessageInput] = useState("");
  const [threadTitleInput, setThreadTitleInput] = useState("");
  const [busyAction, setBusyAction] = useState<string | null>(null);
  const clientRef = useRef<GatewayRpcClient | null>(null);

  const selectedRepo = repos.find((repo) => repo.repo_id === selectedRepoId) ?? null;
  const repoThreads = selectedRepoId ? threadsByRepo[selectedRepoId] ?? [] : [];
  const selectedThread = selectedThreadForRepo(
    selectedRepoId,
    threadsByRepo,
    selectedThreadIdByRepo,
  );

  useEffect(() => {
    return () => {
      clientRef.current?.disconnect();
      clientRef.current = null;
    };
  }, []);

  useEffect(() => {
    if (!pairingSession || pairingSession.status !== "pending") {
      return;
    }

    let cancelled = false;
    const pollCurrentSession = async () => {
      try {
        const response = await pollPairing(serverUrl, pairingSession.pairingId);
        if (!cancelled) {
          applyPairingPoll(response);
        }
      } catch (error) {
        if (!cancelled) {
          const message =
            error instanceof Error ? error.message : "Failed to poll pairing status";
          setLastError(message);
          setConnectionStatus("error");
          pushRuntimeLog("error", `Pairing poll failed: ${message}`);
        }
      }
    };

    void pollCurrentSession();
    const timer = window.setInterval(() => {
      void pollCurrentSession();
    }, 2000);

    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [
    pairingSession,
    pushRuntimeLog,
    serverUrl,
    setConnectionStatus,
    setLastError,
    setPairingSession,
    setBearerToken,
  ]);

  useEffect(() => {
    if (connectionStatus !== "connected" || !selectedRepoId) {
      return;
    }
    void refreshThreads(selectedRepoId).catch((error) => {
      const message = errorMessage(error, "Failed to refresh threads");
      setLastError(message);
      pushRuntimeLog("error", message);
    });
  }, [connectionStatus, selectedRepoId]);

  function applyPairingPoll(response: PairingPollResponse) {
    setPairingSession({
      pairingId: response.pairing_id,
      pairingCode: pairingSession?.pairingCode ?? "",
      expiresAt: response.expires_at,
      status: response.status,
    });

    if (response.status === "approved" && response.token) {
      setBearerToken(response.token);
      setConnectionStatus("idle");
      setLastError(null);
      pushRuntimeLog(
        "info",
        `Pairing approved for ${response.device_label ?? "this device"}. Token stored locally.`,
      );
      return;
    }

    if (response.status === "claimed") {
      setConnectionStatus("idle");
      pushRuntimeLog("info", "Pairing token was already claimed by this client.");
      return;
    }

    if (response.status === "rejected" || response.status === "expired") {
      setConnectionStatus("idle");
      pushRuntimeLog("error", `Pairing ${response.status}.`);
    }
  }

  async function pollPairingNow() {
    if (!pairingSession) {
      return;
    }

    setBusyAction("pairing-poll");
    try {
      applyPairingPoll(await pollPairing(serverUrl, pairingSession.pairingId));
    } catch (error) {
      const message =
        error instanceof Error ? error.message : "Failed to poll pairing status";
      setLastError(message);
      setConnectionStatus("error");
      pushRuntimeLog("error", `Pairing poll failed: ${message}`);
    } finally {
      setBusyAction(null);
    }
  }

  async function ensureThreadLoaded(repoId: string, threadId: string) {
    const threads = useSessionStore.getState().threadsByRepo[repoId] ?? [];
    if (threads.some((thread) => thread.local_thread_id === threadId)) {
      return;
    }
    try {
      await refreshThreads(repoId);
    } catch (error) {
      pushRuntimeLog("error", errorMessage(error, "Failed to refresh threads"));
    }
  }

  function handleGatewayNotification(notification: GatewayNotification) {
    switch (notification.method) {
      case "run.started":
        void ensureThreadLoaded(notification.params.repo_id, notification.params.thread_id);
        updateThreadRun(notification.params.repo_id, notification.params.thread_id, (thread) => ({
          ...thread,
          active_run: {
            turn_id: notification.params.turn_id,
            assistant_text: thread.active_run?.assistant_text ?? "",
            command_output_tail: thread.active_run?.command_output_tail ?? "",
            diff_preview: thread.active_run?.diff_preview ?? "",
            pending_request: null,
          },
        }));
        pushRuntimeLog(
          "info",
          `Run ${shortId(notification.params.turn_id)} started in ${threadLabel(
            notification.params.repo_id,
            notification.params.thread_id,
          )}.`,
        );
        break;
      case "run.delta":
        updateThreadRun(notification.params.repo_id, notification.params.thread_id, (thread) => ({
          ...thread,
          active_run: {
            turn_id: notification.params.turn_id,
            assistant_text: notification.params.assistant_text,
            command_output_tail: thread.active_run?.command_output_tail ?? "",
            diff_preview: thread.active_run?.diff_preview ?? "",
            pending_request: thread.active_run?.pending_request ?? null,
          },
        }));
        break;
      case "run.command_output":
        updateThreadRun(notification.params.repo_id, notification.params.thread_id, (thread) => ({
          ...thread,
          active_run: {
            turn_id: notification.params.turn_id,
            assistant_text: thread.active_run?.assistant_text ?? "",
            command_output_tail: notification.params.command_output_tail,
            diff_preview: thread.active_run?.diff_preview ?? "",
            pending_request: thread.active_run?.pending_request ?? null,
          },
        }));
        break;
      case "run.diff":
        updateThreadRun(notification.params.repo_id, notification.params.thread_id, (thread) => ({
          ...thread,
          active_run: {
            turn_id: notification.params.turn_id,
            assistant_text: thread.active_run?.assistant_text ?? "",
            command_output_tail: thread.active_run?.command_output_tail ?? "",
            diff_preview: notification.params.diff_preview,
            pending_request: thread.active_run?.pending_request ?? null,
          },
        }));
        break;
      case "run.approval_required":
        updateThreadRun(notification.params.repo_id, notification.params.thread_id, (thread) =>
          withPendingRequest(thread, pendingRequestFromEvent(notification.params)),
        );
        pushRuntimeLog(
          "info",
          `Approval required for ${notification.params.kind} in ${threadLabel(
            notification.params.repo_id,
            notification.params.thread_id,
          )}.`,
        );
        break;
      case "run.completed":
      case "run.failed":
        updateThreadRun(notification.params.repo_id, notification.params.thread_id, (thread) => ({
          ...thread,
          active_run: {
            turn_id: notification.params.turn_id,
            assistant_text: notification.params.assistant_text,
            command_output_tail: notification.params.command_output_tail,
            diff_preview: notification.params.diff_preview,
            pending_request: null,
          },
        }));
        pushRuntimeLog(
          notification.method === "run.failed" ? "error" : "info",
          notification.params.error
            ? `${notification.params.status}: ${notification.params.error}`
            : `${notification.params.status} for ${threadLabel(
                notification.params.repo_id,
                notification.params.thread_id,
              )}.`,
        );
        break;
    }
  }

  async function refreshRepos(client = clientRef.current) {
    if (!client) {
      return;
    }

    const response = await client.listRepos();
    setRepos(response.repos);

    const targetRepoId =
      useSessionStore.getState().selectedRepoId ?? response.repos[0]?.repo_id ?? null;
    if (targetRepoId) {
      await refreshThreads(targetRepoId, client);
    }
  }

  async function refreshThreads(repoId: string, client = clientRef.current) {
    if (!client) {
      return;
    }

    const response = await client.listThreads(repoId);
    setThreads(repoId, response.threads);
  }

  async function startPairing() {
    if (!serverUrl.trim() || !deviceLabel.trim()) {
      setLastError("Daemon URL and device label are required.");
      setConnectionStatus("error");
      return;
    }

    setBusyAction("pairing");
    try {
      const response = await requestPairing(serverUrl, deviceLabel.trim());
      setPairingSession({
        pairingId: response.pairing_id,
        pairingCode: response.pairing_code,
        expiresAt: response.expires_at,
        status: "pending",
      });
      setConnectionStatus("pairing");
      setLastError(null);
      pushRuntimeLog(
        "info",
        `Pairing requested. Approve code ${response.pairing_code} on the server.`,
      );
    } catch (error) {
      const message = error instanceof Error ? error.message : "Failed to request pairing";
      setLastError(message);
      setConnectionStatus("error");
      pushRuntimeLog("error", `Pairing request failed: ${message}`);
    } finally {
      setBusyAction(null);
    }
  }

  async function connectGateway() {
    if (!serverUrl.trim() || !bearerToken.trim()) {
      setLastError("Daemon URL and bearer token are required.");
      setConnectionStatus("error");
      return;
    }

    setBusyAction("connect");
    clientRef.current?.disconnect();
    clearRuntimeState();

    const client = new GatewayRpcClient(serverUrl, bearerToken);
    client.onNotification(handleGatewayNotification);
    client.onDisconnect((message) => {
      clientRef.current = null;
      setConnectionStatus("error");
      setLastError(message);
      pushRuntimeLog("error", message);
    });

    try {
      setConnectionStatus("connecting");
      await client.connect();
      clientRef.current = client;
      setConnectionStatus("connected");
      setLastError(null);
      pushRuntimeLog("info", "Connected to APP gateway.");
      await refreshRepos(client);
    } catch (error) {
      const message =
        error instanceof Error ? error.message : "Failed to connect to APP gateway";
      client.disconnect();
      clientRef.current = null;
      setConnectionStatus("error");
      setLastError(message);
      pushRuntimeLog("error", `Connection failed: ${message}`);
    } finally {
      setBusyAction(null);
    }
  }

  function disconnectGateway() {
    clientRef.current?.disconnect();
    clientRef.current = null;
    setConnectionStatus("idle");
    setLastError(null);
    clearRuntimeState();
    pushRuntimeLog("info", "Disconnected from APP gateway.");
  }

  async function handleRepoSelect(repoId: string) {
    selectRepo(repoId);
    if (connectionStatus === "connected") {
      try {
        await refreshThreads(repoId);
      } catch (error) {
        const message = errorMessage(error, "Failed to refresh threads");
        setLastError(message);
        pushRuntimeLog("error", message);
      }
    }
  }

  async function createThread() {
    if (!selectedRepoId || !clientRef.current) {
      return;
    }

    setBusyAction("thread");
    try {
      const thread = await clientRef.current.createThread(
        selectedRepoId,
        threadTitleInput.trim() || undefined,
      );
      upsertThread(selectedRepoId, thread);
      selectThread(selectedRepoId, thread.local_thread_id);
      setThreadTitleInput("");
      pushRuntimeLog("info", `Created thread ${thread.title}.`);
    } catch (error) {
      const message = error instanceof Error ? error.message : "Failed to create thread";
      setLastError(message);
      pushRuntimeLog("error", message);
    } finally {
      setBusyAction(null);
    }
  }

  async function sendMessage() {
    if (
      connectionStatus !== "connected" ||
      !selectedRepoId ||
      !clientRef.current ||
      !messageInput.trim()
    ) {
      return;
    }

    let thread = selectedThread;
    setBusyAction("send");
    try {
      if (!thread) {
        thread = await clientRef.current.createThread(selectedRepoId);
        upsertThread(selectedRepoId, thread);
        selectThread(selectedRepoId, thread.local_thread_id);
      }

      const response = await clientRef.current.sendToThread(
        selectedRepoId,
        thread.local_thread_id,
        messageInput.trim(),
      );
      updateThreadRun(selectedRepoId, thread.local_thread_id, (currentThread) => ({
        ...currentThread,
        active_run: {
          turn_id: response.turn_id ?? currentThread.active_run?.turn_id ?? "",
          assistant_text: "",
          command_output_tail: "",
          diff_preview: "",
          pending_request: null,
        },
      }));
      pushRuntimeLog("info", `Sent message to ${thread.title}.`);
      setMessageInput("");
    } catch (error) {
      const message = error instanceof Error ? error.message : "Failed to send message";
      setLastError(message);
      pushRuntimeLog("error", message);
    } finally {
      setBusyAction(null);
    }
  }

  async function abortRun() {
    if (
      !selectedRepoId ||
      !selectedThread?.active_run?.turn_id ||
      !clientRef.current ||
      connectionStatus !== "connected"
    ) {
      return;
    }

    setBusyAction("abort");
    try {
      await clientRef.current.abortRun(selectedRepoId, selectedThread.active_run.turn_id);
      pushRuntimeLog("info", "Abort requested.");
    } catch (error) {
      const message = error instanceof Error ? error.message : "Failed to abort run";
      setLastError(message);
      pushRuntimeLog("error", message);
    } finally {
      setBusyAction(null);
    }
  }

  async function respondApproval(decision: "accept" | "decline" | "cancel") {
    if (
      !selectedRepoId ||
      !selectedThread?.active_run?.pending_request ||
      !clientRef.current ||
      connectionStatus !== "connected"
    ) {
      return;
    }

    const pendingRequest = selectedThread.active_run.pending_request;
    setBusyAction("approval");
    try {
      await clientRef.current.respondApproval(
        selectedRepoId,
        pendingRequest.request_id,
        decision,
      );
      updateThreadRun(selectedRepoId, selectedThread.local_thread_id, (thread) =>
        withPendingRequest(thread, null),
      );
      pushRuntimeLog("info", `Approval ${decision} sent.`);
    } catch (error) {
      const message =
        error instanceof Error ? error.message : "Failed to respond to approval";
      setLastError(message);
      pushRuntimeLog("error", message);
    } finally {
      setBusyAction(null);
    }
  }

  function threadLabel(repoId: string, threadId: string): string {
    const thread = useSessionStore
      .getState()
      .threadsByRepo[repoId]?.find((item) => item.local_thread_id === threadId);
    return thread?.title ?? `thread ${shortId(threadId)}`;
  }

  return (
    <main className="shell">
      <section className="hero">
        <div className="hero-copy">
          <p className="eyebrow">MyCodex Desktop</p>
          <h1>Remote APP control for repo, thread, run, and approvals.</h1>
          <p className="lede">
            The desktop client talks to the daemon over HTTP and WebSocket, keeps APP
            threads isolated from Telegram, and exposes live run output plus approval
            controls in one surface.
          </p>
        </div>
        <aside className="status-panel">
          <span className={`status-pill status-${connectionStatus}`}>
            {humanConnectionLabel(connectionStatus)}
          </span>
          <dl className="status-list">
            <div>
              <dt>Server</dt>
              <dd>{serverUrl || "unset"}</dd>
            </div>
            <div>
              <dt>Token</dt>
              <dd>{bearerToken ? "stored" : "missing"}</dd>
            </div>
            <div>
              <dt>Selected repo</dt>
              <dd>{selectedRepo?.name ?? "none"}</dd>
            </div>
            <div>
              <dt>Selected thread</dt>
              <dd>{selectedThread?.title ?? "none"}</dd>
            </div>
          </dl>
          {lastError ? <p className="error-text">{lastError}</p> : null}
        </aside>
      </section>

      <section className="dashboard">
        <article className="card">
          <div className="card-header">
            <h2>Connection</h2>
            <span>Daemon + token</span>
          </div>
          <label>
            <span>Daemon URL</span>
            <input
              value={serverUrl}
              onChange={(event) => setServerUrl(event.target.value)}
              placeholder="http://127.0.0.1:3940"
            />
          </label>
          <label>
            <span>Device label</span>
            <input
              value={deviceLabel}
              onChange={(event) => setDeviceLabel(event.target.value)}
              placeholder="MyCodex Desktop"
            />
          </label>
          <label>
            <span>Bearer token</span>
            <input
              value={bearerToken}
              onChange={(event) => setBearerToken(event.target.value)}
              placeholder="mcx_..."
            />
          </label>
          <div className="card-actions">
            <button
              disabled={busyAction === "connect" || connectionStatus === "connected"}
              onClick={() => void connectGateway()}
            >
              {connectionStatus === "connected" ? "Connected" : "Connect"}
            </button>
            <button className="ghost" onClick={disconnectGateway}>
              Disconnect
            </button>
            <button
              className="ghost"
              disabled={connectionStatus !== "connected"}
              onClick={() =>
                void refreshRepos().catch((error) => {
                  const message = errorMessage(error, "Failed to refresh repos");
                  setLastError(message);
                  pushRuntimeLog("error", message);
                })
              }
            >
              Refresh
            </button>
          </div>
        </article>

        <article className="card">
          <div className="card-header">
            <h2>Pairing</h2>
            <span>CLI approved</span>
          </div>
          <p className="muted">
            Request a short pairing code from the daemon, approve it on the server, and
            keep polling until the bearer token arrives.
          </p>
          <div className="pairing-box">
            <div>
              <strong>Code</strong>
              <span>{pairingSession?.pairingCode ?? "none"}</span>
            </div>
            <div>
              <strong>Status</strong>
              <span>{pairingSession?.status ?? "idle"}</span>
            </div>
            <div>
              <strong>Expires</strong>
              <span>{formatTimestamp(pairingSession?.expiresAt)}</span>
            </div>
          </div>
          <div className="card-actions">
            <button disabled={busyAction === "pairing"} onClick={() => void startPairing()}>
              Start pairing
            </button>
            <button
              className="ghost"
              disabled={!pairingSession || busyAction === "pairing-poll"}
              onClick={() => void pollPairingNow()}
            >
              Poll now
            </button>
          </div>
          <p className="hint">
            Approve with <code>mycodex app pairing approve &lt;CODE&gt;</code>.
          </p>
        </article>

        <article className="card tall">
          <div className="card-header">
            <h2>Repos</h2>
            <span>{repos.length} loaded</span>
          </div>
          <div className="stack">
            {repos.length === 0 ? (
              <p className="empty">Connect to the daemon to load repos.</p>
            ) : (
              repos.map((repo) => (
                <button
                  key={repo.repo_id}
                  className={`list-row ${selectedRepoId === repo.repo_id ? "selected" : ""}`}
                  onClick={() => void handleRepoSelect(repo.repo_id)}
                >
                  <strong>{repo.name}</strong>
                  <span>{describeRepo(repo)}</span>
                </button>
              ))
            )}
          </div>
        </article>

        <article className="card tall">
          <div className="card-header">
            <h2>Threads</h2>
            <span>{selectedRepo?.name ?? "Select a repo"}</span>
          </div>
          <label>
            <span>New thread title</span>
            <input
              value={threadTitleInput}
              onChange={(event) => setThreadTitleInput(event.target.value)}
              placeholder="Optional title"
            />
          </label>
          <div className="card-actions">
            <button
              disabled={!selectedRepoId || busyAction === "thread" || connectionStatus !== "connected"}
              onClick={() => void createThread()}
            >
              New thread
            </button>
            <button
              className="ghost"
              disabled={!selectedRepoId || connectionStatus !== "connected"}
              onClick={() =>
                selectedRepoId
                  ? void refreshThreads(selectedRepoId).catch((error) => {
                      const message = errorMessage(error, "Failed to refresh threads");
                      setLastError(message);
                      pushRuntimeLog("error", message);
                    })
                  : undefined
              }
            >
              Refresh list
            </button>
          </div>
          <div className="stack">
            {repoThreads.length > 0 ? (
              repoThreads.map((thread) => (
                <button
                  key={thread.local_thread_id}
                  className={`list-row ${
                    selectedThread?.local_thread_id === thread.local_thread_id ? "selected" : ""
                  }`}
                  onClick={() =>
                    selectedRepoId ? selectThread(selectedRepoId, thread.local_thread_id) : undefined
                  }
                >
                  <strong>{thread.title}</strong>
                  <span>
                    {thread.active_run
                      ? `Run ${shortId(thread.active_run.turn_id)}`
                      : thread.status}
                  </span>
                </button>
              ))
            ) : (
              <p className="empty">No APP threads for this repo yet.</p>
            )}
          </div>
        </article>

        <article className="card surface output-surface">
          <div className="card-header">
            <h2>Run output</h2>
            <span>{selectedThread?.title ?? "No thread selected"}</span>
          </div>
          {selectedThread ? (
            <>
              <div className="thread-meta">
                <div>
                  <strong>Thread</strong>
                  <span>{shortId(selectedThread.codex_thread_id)}</span>
                </div>
                <div>
                  <strong>Last used</strong>
                  <span>{formatTimestamp(selectedThread.last_used_at)}</span>
                </div>
                <div>
                  <strong>Run</strong>
                  <span>{selectedThread.active_run ? shortId(selectedThread.active_run.turn_id) : "idle"}</span>
                </div>
              </div>

              <div className="composer">
                <textarea
                  rows={4}
                  value={messageInput}
                  onChange={(event) => setMessageInput(event.target.value)}
                  onKeyDown={(event) => {
                    if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
                      event.preventDefault();
                      void sendMessage();
                    }
                  }}
                  placeholder="Send a message into the selected APP thread..."
                />
                <div className="card-actions">
                  <button
                    disabled={
                      connectionStatus !== "connected" ||
                      !selectedRepoId ||
                      !messageInput.trim() ||
                      busyAction === "send"
                    }
                    onClick={() => void sendMessage()}
                  >
                    Send
                  </button>
                  <button
                    className="ghost"
                    disabled={!selectedThread.active_run?.turn_id || busyAction === "abort"}
                    onClick={() => void abortRun()}
                  >
                    Abort run
                  </button>
                </div>
              </div>

              <div className="output-grid">
                <section className="output-panel">
                  <h3>Assistant</h3>
                  <pre>{selectedThread.active_run?.assistant_text || "No output yet."}</pre>
                </section>
                <section className="output-panel">
                  <h3>Command output</h3>
                  <pre>
                    {selectedThread.active_run?.command_output_tail || "No command output yet."}
                  </pre>
                </section>
                <section className="output-panel full">
                  <h3>Diff preview</h3>
                  <pre>{selectedThread.active_run?.diff_preview || "No diff yet."}</pre>
                </section>
              </div>
            </>
          ) : (
            <p className="empty">Select a repo and thread to start interacting.</p>
          )}
        </article>

        <article className="card">
          <div className="card-header">
            <h2>Approval</h2>
            <span>APP route only</span>
          </div>
          {selectedThread?.active_run?.pending_request ? (
            <>
              <div className="approval-summary">
                <p>
                  <strong>Kind</strong>
                  <span>{selectedThread.active_run.pending_request.kind}</span>
                </p>
                <p>
                  <strong>Target</strong>
                  <span>{describePendingRequest(selectedThread.active_run.pending_request)}</span>
                </p>
                <p>
                  <strong>Reason</strong>
                  <span>{selectedThread.active_run.pending_request.reason || "none"}</span>
                </p>
              </div>
              {selectedThread.active_run.pending_request.kind === "file" ? (
                <pre className="approval-diff">
                  {selectedThread.active_run.pending_request.diff_preview || "No diff preview."}
                </pre>
              ) : null}
              <div className="card-actions">
                <button
                  disabled={busyAction === "approval"}
                  onClick={() => void respondApproval("accept")}
                >
                  Accept
                </button>
                <button
                  className="ghost"
                  disabled={busyAction === "approval"}
                  onClick={() => void respondApproval("decline")}
                >
                  Decline
                </button>
                <button
                  className="ghost"
                  disabled={busyAction === "approval"}
                  onClick={() => void respondApproval("cancel")}
                >
                  Cancel
                </button>
              </div>
            </>
          ) : (
            <p className="empty">No approval is pending for the selected thread.</p>
          )}
        </article>

        <article className="card">
          <div className="card-header">
            <h2>Activity</h2>
            <span>Most recent first</span>
          </div>
          <div className="log">
            {runtimeLog.length === 0 ? (
              <p className="empty">No desktop-side activity yet.</p>
            ) : (
              runtimeLog.map((entry) => (
                <div key={entry.id} className={`log-entry log-${entry.level}`}>
                  <div className="log-heading">
                    <strong>{entry.level}</strong>
                    <span>{formatTimestamp(entry.createdAt)}</span>
                  </div>
                  <p>{entry.message}</p>
                </div>
              ))
            )}
          </div>
        </article>
      </section>
    </main>
  );
}
