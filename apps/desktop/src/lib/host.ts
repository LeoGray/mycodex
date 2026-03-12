import { invoke } from "@tauri-apps/api/core";

export type HostRuntimeStatus =
  | "stopped"
  | "starting"
  | "running"
  | "stopping"
  | "crashed";

export type HostNetworkMode = "local_only" | "lan";

export type HostLogEntry = {
  id: string;
  level: string;
  source: string;
  message: string;
  createdAt: string;
};

export type HostConfigSnapshot = {
  networkMode: HostNetworkMode;
  port: number;
  bindAddress: string;
  lanUrl: string | null;
  workspaceRoot: string;
  stateDir: string;
  configPath: string;
  logPath: string;
  codexBin: string;
  binaryPath: string | null;
  workingDirectory: string | null;
  telegramEnabled: boolean;
};

export type HostStatusSnapshot = {
  status: HostRuntimeStatus;
  pid: number | null;
  lastError: string | null;
  config: HostConfigSnapshot;
  recentLogs: HostLogEntry[];
};

export type LocalHostConnection = {
  serverUrl: string;
  bearerToken: string;
  deviceLabel: string;
};

export type HostConfigUpdate = {
  networkMode?: HostNetworkMode;
  port?: number;
  workspaceRoot?: string;
  stateDir?: string;
  codexBin?: string;
  telegramBotToken?: string;
  binaryPath?: string | null;
  workingDirectory?: string | null;
};

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function unavailableStatus(): HostStatusSnapshot {
  return {
    status: "stopped",
    pid: null,
    lastError: "Host controls are only available inside the desktop app shell.",
    config: {
      networkMode: "local_only",
      port: 3940,
      bindAddress: "127.0.0.1:3940",
      lanUrl: null,
      workspaceRoot: "",
      stateDir: "",
      configPath: "",
      logPath: "",
      codexBin: "codex",
      binaryPath: null,
      workingDirectory: null,
      telegramEnabled: false,
    },
    recentLogs: [],
  };
}

export async function getHostStatus(): Promise<HostStatusSnapshot> {
  if (!isTauriRuntime()) {
    return unavailableStatus();
  }
  return invoke<HostStatusSnapshot>("get_host_status");
}

export async function updateHostConfig(
  request: HostConfigUpdate,
): Promise<HostStatusSnapshot> {
  if (!isTauriRuntime()) {
    throw new Error("Host controls are only available inside the desktop app shell.");
  }
  return invoke<HostStatusSnapshot>("update_host_config", { request });
}

export async function startHost(
  request?: HostConfigUpdate,
): Promise<HostStatusSnapshot> {
  if (!isTauriRuntime()) {
    throw new Error("Host controls are only available inside the desktop app shell.");
  }
  return invoke<HostStatusSnapshot>("start_host", { request: request ?? null });
}

export async function stopHost(): Promise<HostStatusSnapshot> {
  if (!isTauriRuntime()) {
    return unavailableStatus();
  }
  return invoke<HostStatusSnapshot>("stop_host");
}

export async function restartHost(
  request?: HostConfigUpdate,
): Promise<HostStatusSnapshot> {
  if (!isTauriRuntime()) {
    throw new Error("Host controls are only available inside the desktop app shell.");
  }
  return invoke<HostStatusSnapshot>("restart_host", { request: request ?? null });
}

export async function readHostLogs(): Promise<HostLogEntry[]> {
  if (!isTauriRuntime()) {
    return [];
  }
  return invoke<HostLogEntry[]>("read_host_logs");
}

export async function issueLocalHostConnection(): Promise<LocalHostConnection> {
  if (!isTauriRuntime()) {
    throw new Error("Host controls are only available inside the desktop app shell.");
  }
  return invoke<LocalHostConnection>("issue_local_host_connection");
}
