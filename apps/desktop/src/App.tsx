import { useEffect, useRef, useState } from "react";

import { HostView } from "./components/HostView";
import { SettingsView } from "./components/SettingsView";
import { WorkbenchView } from "./components/WorkbenchView";
import { GatewayRpcClient, pollPairing, requestPairing } from "./lib/gateway";
import {
  getHostStatus,
  issueLocalHostConnection,
  restartHost,
  startHost,
  stopHost,
  updateHostConfig,
} from "./lib/host";
import {
  selectedThreadForRepo,
  useSessionStore,
  withPendingRequest,
} from "./store/session";
import type { AppMode, ConnectionStatus, RuntimeLog } from "./store/session";
import type { HostConfigUpdate, HostStatusSnapshot } from "./lib/host";
import type {
  AppPendingRequest,
  AppThreadSummary,
  GatewayNotification,
  PairingPollResponse,
  RunApprovalRequiredEvent,
} from "./types/gateway";

const MOBILE_BREAKPOINT = 860;

function humanConnectionLabel(status: ConnectionStatus): string {
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

function shortId(value: string | null | undefined): string {
  return value ? value.slice(0, 8) : "n/a";
}

function modeLabel(mode: AppMode): string {
  switch (mode) {
    case "local_host":
      return "Local Host";
    case "local_host_client":
      return "Local Host + Client";
    default:
      return "Remote Client";
  }
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

function errorMessage(error: unknown, fallback: string): string {
  if (error instanceof Error) {
    return error.message;
  }
  if (typeof error === "string" && error.trim()) {
    return error;
  }
  if (
    typeof error === "object" &&
    error !== null &&
    "message" in error &&
    typeof (error as { message?: unknown }).message === "string"
  ) {
    return (error as { message: string }).message;
  }
  return fallback;
}

function hostLogsFromSnapshot(snapshot: HostStatusSnapshot): RuntimeLog[] {
  return snapshot.recentLogs.map((entry) => ({
    id: entry.id,
    level: entry.level === "error" ? "error" : "info",
    message: `[${entry.source}] ${entry.message}`,
    createdAt: entry.createdAt,
  }));
}

function hostConfigToUpdate(config: {
  networkMode: "local_only" | "lan";
  port: number;
  workspaceRoot: string;
  stateDir: string;
  codexBin: string;
  binaryPath: string;
  workingDirectory: string;
}): HostConfigUpdate {
  return {
    networkMode: config.networkMode,
    port: config.port,
    workspaceRoot: config.workspaceRoot,
    stateDir: config.stateDir,
    codexBin: config.codexBin,
    binaryPath: config.binaryPath || null,
    workingDirectory: config.workingDirectory || null,
  };
}

export default function App() {
  const {
    mode,
    host,
    remote,
    workbench,
    setMode,
    setActivePage,
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
    setHostRuntimeStatus,
    setHostLastError,
    setHostConfig,
    replaceHostLogs,
  } = useSessionStore();

  const { selectedMode, activePage } = mode;
  const { runtimeStatus: hostRuntimeStatus, lastError: hostLastError, config: hostConfig } = host;
  const { serverUrl, deviceLabel, bearerToken, connectionStatus, lastError, pairingSession } =
    remote;
  const { repos, threadsByRepo, selectedRepoId, selectedThreadIdByRepo, runtimeLog } =
    workbench;

  const [messageInput, setMessageInput] = useState("");
  const [threadTitleInput, setThreadTitleInput] = useState("");
  const [busyAction, setBusyAction] = useState<string | null>(null);
  const [isCompactLayout, setIsCompactLayout] = useState(false);
  const [hostDraft, setHostDraft] = useState({
    networkMode: hostConfig.networkMode,
    port: hostConfig.port,
    workspaceRoot: hostConfig.workspaceRoot,
    stateDir: hostConfig.stateDir,
    codexBin: hostConfig.codexBin,
    binaryPath: hostConfig.binaryPath,
    workingDirectory: hostConfig.workingDirectory,
  });
  const clientRef = useRef<GatewayRpcClient | null>(null);
  const modeBootstrapRef = useRef<AppMode | null>(null);
  const localHostControlsAvailable =
    typeof window !== "undefined" &&
    "__TAURI_INTERNALS__" in window &&
    !/Android|iPhone|iPad|iPod/i.test(window.navigator.userAgent);

  const selectedRepo = repos.find((repo) => repo.repo_id === selectedRepoId) ?? null;
  const repoThreads = selectedRepoId ? threadsByRepo[selectedRepoId] ?? [] : [];
  const selectedThread = selectedThreadForRepo(selectedRepoId, workbench);

  useEffect(() => {
    setHostDraft({
      networkMode: hostConfig.networkMode,
      port: hostConfig.port,
      workspaceRoot: hostConfig.workspaceRoot,
      stateDir: hostConfig.stateDir,
      codexBin: hostConfig.codexBin,
      binaryPath: hostConfig.binaryPath,
      workingDirectory: hostConfig.workingDirectory,
    });
  }, [
    hostConfig.binaryPath,
    hostConfig.codexBin,
    hostConfig.networkMode,
    hostConfig.port,
    hostConfig.stateDir,
    hostConfig.workingDirectory,
    hostConfig.workspaceRoot,
  ]);

  useEffect(() => {
    const updateLayout = () => {
      setIsCompactLayout(window.innerWidth <= MOBILE_BREAKPOINT);
    };

    updateLayout();
    window.addEventListener("resize", updateLayout);
    return () => {
      window.removeEventListener("resize", updateLayout);
    };
  }, []);

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

  useEffect(() => {
    if (!localHostControlsAvailable && selectedMode !== "remote_client") {
      setMode("remote_client");
      setActivePage("workbench");
      setLastError("Local Host modes are only available inside the desktop app shell.");
      return;
    }

    if (modeBootstrapRef.current === selectedMode) {
      return;
    }

    void bootstrapMode(selectedMode);
  }, [
    localHostControlsAvailable,
    selectedMode,
    setActivePage,
    setLastError,
    setMode,
  ]);

  function syncHostSnapshot(snapshot: HostStatusSnapshot) {
    setHostRuntimeStatus(snapshot.status);
    setHostLastError(snapshot.lastError);
    setHostConfig({
      networkMode: snapshot.config.networkMode,
      port: snapshot.config.port,
      bindAddress: snapshot.config.bindAddress,
      lanAddress: snapshot.config.lanUrl,
      workspaceRoot: snapshot.config.workspaceRoot,
      stateDir: snapshot.config.stateDir,
      configPath: snapshot.config.configPath,
      logPath: snapshot.config.logPath,
      codexBin: snapshot.config.codexBin,
      binaryPath: snapshot.config.binaryPath ?? "",
      workingDirectory: snapshot.config.workingDirectory ?? "",
      telegramEnabled: snapshot.config.telegramEnabled,
    });
    replaceHostLogs(hostLogsFromSnapshot(snapshot));
  }

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
    const threads = useSessionStore.getState().workbench.threadsByRepo[repoId] ?? [];
    if (threads.some((thread) => thread.local_thread_id === threadId)) {
      return;
    }
    try {
      await refreshThreads(repoId);
    } catch (error) {
      pushRuntimeLog("error", errorMessage(error, "Failed to refresh threads"));
    }
  }

  function threadLabel(repoId: string, threadId: string): string {
    const thread = useSessionStore
      .getState()
      .workbench.threadsByRepo[repoId]?.find((item) => item.local_thread_id === threadId);
    return thread?.title ?? `thread ${shortId(threadId)}`;
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
      useSessionStore.getState().workbench.selectedRepoId ?? response.repos[0]?.repo_id ?? null;
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

  async function connectGatewayWithCredentials(
    targetServerUrl: string,
    targetBearerToken: string,
  ) {
    if (!targetServerUrl.trim() || !targetBearerToken.trim()) {
      setLastError("Daemon URL and bearer token are required.");
      setConnectionStatus("error");
      return;
    }

    setBusyAction("connect");
    clientRef.current?.disconnect();
    clearRuntimeState();

    const client = new GatewayRpcClient(targetServerUrl, targetBearerToken);
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

  async function connectGateway() {
    await connectGatewayWithCredentials(serverUrl, bearerToken);
  }

  function disconnectGateway() {
    clientRef.current?.disconnect();
    clientRef.current = null;
    setConnectionStatus("idle");
    setLastError(null);
    clearRuntimeState();
    pushRuntimeLog("info", "Disconnected from APP gateway.");
  }

  async function bootstrapMode(nextMode: AppMode) {
    if (nextMode === "remote_client") {
      modeBootstrapRef.current = nextMode;
      return;
    }

    setBusyAction("mode-bootstrap");
    try {
      const desiredConfig = hostConfigToUpdate(useSessionStore.getState().host.config);
      const currentHostStatus = await updateHostConfig(desiredConfig);
      syncHostSnapshot(currentHostStatus);

      const hostSnapshot =
        currentHostStatus.status === "running"
          ? currentHostStatus
          : await startHost(desiredConfig);
      syncHostSnapshot(hostSnapshot);

      if (nextMode === "local_host_client") {
        await connectToLocalHostClient();
        setActivePage("workbench");
      } else {
        disconnectGateway();
        setActivePage("host");
      }

      modeBootstrapRef.current = nextMode;
    } catch (error) {
      const message = errorMessage(error, "Failed to prepare local host mode");
      setLastError(message);
      setConnectionStatus("error");
      pushRuntimeLog("error", `Local mode bootstrap failed: ${message}`);
      modeBootstrapRef.current = null;
    } finally {
      setBusyAction(null);
    }
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

  function hostDraftToUpdate(): HostConfigUpdate {
    return hostConfigToUpdate(hostDraft);
  }

  async function connectToLocalHostClient() {
    const localConnection = await issueLocalHostConnection();
    setServerUrl(localConnection.serverUrl);
    setDeviceLabel(localConnection.deviceLabel);
    setBearerToken(localConnection.bearerToken);

    for (let attempt = 0; attempt < 8; attempt += 1) {
      await connectGatewayWithCredentials(
        localConnection.serverUrl,
        localConnection.bearerToken,
      );
      if (useSessionStore.getState().remote.connectionStatus === "connected") {
        return;
      }
      await new Promise((resolve) => {
        window.setTimeout(resolve, 750);
      });
    }

    throw new Error("Local host started, but the client could not connect yet.");
  }

  async function refreshHostStatus() {
    try {
      syncHostSnapshot(await getHostStatus());
    } catch (error) {
      setHostLastError(errorMessage(error, "Failed to refresh host status"));
    }
  }

  async function applyHostConfig() {
    try {
      const request = hostDraftToUpdate();
      const snapshot =
        hostRuntimeStatus === "running"
          ? await restartHost(request)
          : await updateHostConfig(request);
      syncHostSnapshot(snapshot);
      if (selectedMode === "local_host_client" && snapshot.status === "running") {
        await connectToLocalHostClient();
      }
    } catch (error) {
      setHostLastError(errorMessage(error, "Failed to update host config"));
    }
  }

  async function startLocalHostFromView() {
    setBusyAction("host-start");
    try {
      const snapshot = await startHost(hostDraftToUpdate());
      syncHostSnapshot(snapshot);
      if (selectedMode === "local_host_client") {
        await connectToLocalHostClient();
      }
    } catch (error) {
      const message = errorMessage(error, "Failed to start local host");
      setHostLastError(message);
      pushRuntimeLog("error", message);
    } finally {
      setBusyAction(null);
    }
  }

  async function stopLocalHostFromView() {
    try {
      if (selectedMode === "local_host_client") {
        disconnectGateway();
      }
      syncHostSnapshot(await stopHost());
    } catch (error) {
      setHostLastError(errorMessage(error, "Failed to stop local host"));
    }
  }

  async function restartLocalHostFromView() {
    setBusyAction("host-restart");
    try {
      const snapshot = await restartHost(hostDraftToUpdate());
      syncHostSnapshot(snapshot);
      if (selectedMode === "local_host_client") {
        await connectToLocalHostClient();
      }
    } catch (error) {
      const message = errorMessage(error, "Failed to restart local host");
      setHostLastError(message);
      pushRuntimeLog("error", message);
    } finally {
      setBusyAction(null);
    }
  }

  async function handleModeSelection(nextMode: AppMode) {
    if (nextMode !== "remote_client" && !localHostControlsAvailable) {
      setLastError("Local Host modes are only available inside the desktop app shell.");
      return;
    }

    setMode(nextMode);
    modeBootstrapRef.current = null;

    if (nextMode === "remote_client") {
      setActivePage("workbench");
      disconnectGateway();
      try {
        syncHostSnapshot(await stopHost());
      } catch (error) {
        pushRuntimeLog("error", errorMessage(error, "Failed to stop local host"));
      }
      return;
    }

    if (nextMode === "local_host") {
      disconnectGateway();
      setActivePage("host");
      return;
    }

    setActivePage("workbench");
  }

  const desktopPage = activePage === "host" ? "host" : activePage;
  const mobilePage = activePage === "settings" ? "settings" : "workbench";
  const currentPage = isCompactLayout ? mobilePage : desktopPage;
  const topbarCopy =
    currentPage === "host"
      ? {
          title: "Local host on this Mac",
          subtitle: "Run MyCodex locally, control access, and share it on your LAN when you want to.",
        }
      : currentPage === "settings"
        ? {
            title: "Connection settings",
            subtitle: "Server URL, pairing, tokens, and diagnostics live here instead of the main workbench.",
          }
        : {
            title: "Workspace",
            subtitle: "Switch repos, continue threads, and send the next task without leaving the main work area.",
          };

  return (
    <main className="shell app-shell">
      <header className="app-topbar">
        <div>
          <p className="eyebrow">MyCodex App</p>
          <h1 className="app-title">{topbarCopy.title}</h1>
          <p className="app-subtitle">{topbarCopy.subtitle}</p>
        </div>
        <div className="topbar-status">
          <span className={`status-pill status-${connectionStatus}`}>
            {humanConnectionLabel(connectionStatus)}
          </span>
          <span className="status-pill status-connecting">{modeLabel(selectedMode)}</span>
        </div>
      </header>

      <div className="app-frame">
        {!isCompactLayout ? (
          <aside className="app-sidebar">
            <button
              type="button"
              className={`nav-link ${desktopPage === "workbench" ? "active" : ""}`}
              onClick={() => setActivePage("workbench")}
            >
              Workbench
            </button>
            <button
              type="button"
              className={`nav-link ${desktopPage === "settings" ? "active" : ""}`}
              onClick={() => setActivePage("settings")}
            >
              Settings
            </button>
            {localHostControlsAvailable ? (
              <button
                type="button"
                className={`nav-link ${desktopPage === "host" ? "active" : ""}`}
                onClick={() => setActivePage("host")}
              >
                Host
              </button>
            ) : null}
          </aside>
        ) : null}

        <section className="app-main">
          {(isCompactLayout ? mobilePage : desktopPage) === "workbench" ? (
            <WorkbenchView
              compact={isCompactLayout}
              workspaceLabel={
                selectedMode === "remote_client" ? "Remote Workspace" : "Local Workspace"
              }
              connectionStatus={connectionStatus}
              repos={repos}
              repoThreads={repoThreads}
              selectedRepoId={selectedRepoId}
              selectedRepoName={selectedRepo?.name ?? null}
              selectedThread={selectedThread}
              runtimeLog={runtimeLog}
              threadTitleInput={threadTitleInput}
              messageInput={messageInput}
              busyAction={busyAction}
              onSelectRepo={(repoId) => void handleRepoSelect(repoId)}
              onSelectThread={(threadId) => {
                if (!selectedRepoId) {
                  return;
                }
                selectThread(selectedRepoId, threadId);
              }}
              onThreadTitleInputChange={setThreadTitleInput}
              onMessageInputChange={setMessageInput}
              onCreateThread={() => void createThread()}
              onRefreshThreads={() =>
                selectedRepoId
                  ? void refreshThreads(selectedRepoId).catch((error) => {
                      const message = errorMessage(error, "Failed to refresh threads");
                      setLastError(message);
                      pushRuntimeLog("error", message);
                    })
                  : undefined
              }
              onSendMessage={() => void sendMessage()}
              onAbortRun={() => void abortRun()}
              onRespondApproval={(decision) => void respondApproval(decision)}
            />
          ) : null}

          {(isCompactLayout ? mobilePage : desktopPage) === "settings" ? (
            <SettingsView
              mode={selectedMode}
              allowLocalModes={localHostControlsAvailable}
              connectionStatus={connectionStatus}
              serverUrl={serverUrl}
              deviceLabel={deviceLabel}
              bearerToken={bearerToken}
              pairingSession={pairingSession}
              lastError={lastError}
              hostStatusLabel={hostRuntimeStatus}
              selectedRepoName={selectedRepo?.name ?? null}
              selectedThreadTitle={selectedThread?.title ?? null}
              busyAction={busyAction}
              onServerUrlChange={setServerUrl}
              onDeviceLabelChange={setDeviceLabel}
              onBearerTokenChange={setBearerToken}
              onModeChange={(modeValue) => void handleModeSelection(modeValue)}
              onConnect={() => void connectGateway()}
              onDisconnect={disconnectGateway}
              onRefresh={() =>
                void refreshRepos().catch((error) => {
                  const message = errorMessage(error, "Failed to refresh repos");
                  setLastError(message);
                  pushRuntimeLog("error", message);
                })
              }
              onStartPairing={() => void startPairing()}
              onPollPairing={() => void pollPairingNow()}
            />
          ) : null}

          {!isCompactLayout && desktopPage === "host" ? (
            <HostView
              status={hostRuntimeStatus}
              lastError={hostLastError}
              hostDraft={hostDraft}
              bindAddress={hostConfig.bindAddress}
              lanAddress={hostConfig.lanAddress}
              configPath={hostConfig.configPath}
              logPath={hostConfig.logPath}
              telegramEnabled={hostConfig.telegramEnabled}
              recentLogs={host.recentLogs}
              busyAction={busyAction}
              onDraftChange={setHostDraft}
              onApplyConfig={() => void applyHostConfig()}
              onStartHost={() => void startLocalHostFromView()}
              onStopHost={() => void stopLocalHostFromView()}
              onRestartHost={() => void restartLocalHostFromView()}
              onRefreshHost={() => void refreshHostStatus()}
            />
          ) : null}
        </section>
      </div>

      {isCompactLayout ? (
        <nav className="mobile-tabs" aria-label="Primary pages">
          <button
            type="button"
            className={`nav-link ${mobilePage === "workbench" ? "active" : ""}`}
            onClick={() => setActivePage("workbench")}
          >
            Workbench
          </button>
          <button
            type="button"
            className={`nav-link ${mobilePage === "settings" ? "active" : ""}`}
            onClick={() => setActivePage("settings")}
          >
            Settings
          </button>
        </nav>
      ) : null}
    </main>
  );
}
