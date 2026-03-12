import { create } from "zustand";
import { persist } from "zustand/middleware";

import type {
  AppPendingRequest,
  AppRepoSummary,
  AppThreadSummary,
  PairingPollStatus,
} from "../types/gateway";

export type AppMode = "remote_client" | "local_host" | "local_host_client";

export type ConnectionStatus =
  | "idle"
  | "pairing"
  | "connecting"
  | "connected"
  | "error";

export type PairingSession = {
  pairingId: string;
  pairingCode: string;
  expiresAt: string;
  status: PairingPollStatus;
};

export type RuntimeLog = {
  id: string;
  level: "info" | "error";
  message: string;
  createdAt: string;
};

export type ModeState = {
  selectedMode: AppMode;
  activePage: "workbench" | "settings" | "host";
};

export type RemoteConnectionState = {
  serverUrl: string;
  deviceLabel: string;
  bearerToken: string;
  connectionStatus: ConnectionStatus;
  lastError: string | null;
  pairingSession: PairingSession | null;
};

export type HostNetworkMode = "local_only" | "lan";
export type HostRuntimeStatus =
  | "stopped"
  | "starting"
  | "running"
  | "stopping"
  | "crashed";

export type HostState = {
  runtimeStatus: HostRuntimeStatus;
  lastError: string | null;
  recentLogs: RuntimeLog[];
  config: {
    networkMode: HostNetworkMode;
    port: number;
    bindAddress: string;
    lanAddress: string | null;
    workspaceRoot: string;
    stateDir: string;
    configPath: string;
    logPath: string;
    codexBin: string;
    binaryPath: string;
    workingDirectory: string;
    telegramEnabled: boolean;
  };
};

export type WorkbenchState = {
  repos: AppRepoSummary[];
  threadsByRepo: Record<string, AppThreadSummary[]>;
  selectedRepoId: string | null;
  selectedThreadIdByRepo: Record<string, string | null>;
  runtimeLog: RuntimeLog[];
};

type SessionState = {
  mode: ModeState;
  remote: RemoteConnectionState;
  host: HostState;
  workbench: WorkbenchState;
  setMode: (value: AppMode) => void;
  setActivePage: (value: ModeState["activePage"]) => void;
  setServerUrl: (value: string) => void;
  setDeviceLabel: (value: string) => void;
  setBearerToken: (value: string) => void;
  setConnectionStatus: (value: ConnectionStatus) => void;
  setLastError: (value: string | null) => void;
  setPairingSession: (value: PairingSession | null) => void;
  setRepos: (repos: AppRepoSummary[]) => void;
  setThreads: (repoId: string, threads: AppThreadSummary[]) => void;
  upsertThread: (repoId: string, thread: AppThreadSummary) => void;
  selectRepo: (repoId: string | null) => void;
  selectThread: (repoId: string, threadId: string | null) => void;
  updateThreadRun: (
    repoId: string,
    threadId: string,
    updater: (thread: AppThreadSummary) => AppThreadSummary,
  ) => void;
  clearRuntimeState: () => void;
  pushRuntimeLog: (level: "info" | "error", message: string) => void;
  setHostRuntimeStatus: (value: HostRuntimeStatus) => void;
  setHostLastError: (value: string | null) => void;
  setHostConfig: (value: Partial<HostState["config"]>) => void;
  replaceHostLogs: (value: RuntimeLog[]) => void;
  pushHostLog: (level: "info" | "error", message: string) => void;
};

const initialModeState: ModeState = {
  selectedMode: "remote_client",
  activePage: "workbench",
};

const initialRemoteState: RemoteConnectionState = {
  serverUrl: "http://127.0.0.1:3940",
  deviceLabel: "MyCodex App",
  bearerToken: "",
  connectionStatus: "idle",
  lastError: null,
  pairingSession: null,
};

const initialHostState: HostState = {
  runtimeStatus: "stopped",
  lastError: null,
  recentLogs: [],
  config: {
    networkMode: "local_only",
    port: 3940,
    bindAddress: "127.0.0.1:3940",
    lanAddress: null,
    workspaceRoot: "",
    stateDir: "",
    configPath: "",
    logPath: "",
    codexBin: "",
    binaryPath: "",
    workingDirectory: "",
    telegramEnabled: false,
  },
};

const initialWorkbenchState: WorkbenchState = {
  repos: [],
  threadsByRepo: {},
  selectedRepoId: null,
  selectedThreadIdByRepo: {},
  runtimeLog: [],
};

function preferThreadSelection(
  currentThreadId: string | null | undefined,
  threads: AppThreadSummary[],
): string | null {
  if (currentThreadId && threads.some((thread) => thread.local_thread_id === currentThreadId)) {
    return currentThreadId;
  }

  const runningThread = threads.find((thread) => thread.active_run);
  return runningThread?.local_thread_id ?? threads[0]?.local_thread_id ?? null;
}

function appendRuntimeLog(
  currentLog: RuntimeLog[],
  level: RuntimeLog["level"],
  message: string,
): RuntimeLog[] {
  return [
    {
      id: `${Date.now()}-${currentLog.length + 1}`,
      level,
      message,
      createdAt: new Date().toISOString(),
    },
    ...currentLog,
  ].slice(0, 24);
}

export const useSessionStore = create<SessionState>()(
  persist(
    (set) => ({
      mode: initialModeState,
      remote: initialRemoteState,
      host: initialHostState,
      workbench: initialWorkbenchState,
      setMode: (value) =>
        set((state) => ({
          mode: {
            ...state.mode,
            selectedMode: value,
          },
        })),
      setActivePage: (value) =>
        set((state) => ({
          mode: {
            ...state.mode,
            activePage: value,
          },
        })),
      setServerUrl: (value) =>
        set((state) => ({
          remote: {
            ...state.remote,
            serverUrl: value,
          },
        })),
      setDeviceLabel: (value) =>
        set((state) => ({
          remote: {
            ...state.remote,
            deviceLabel: value,
          },
        })),
      setBearerToken: (value) =>
        set((state) => ({
          remote: {
            ...state.remote,
            bearerToken: value,
          },
        })),
      setConnectionStatus: (value) =>
        set((state) => ({
          remote: {
            ...state.remote,
            connectionStatus: value,
          },
        })),
      setLastError: (value) =>
        set((state) => ({
          remote: {
            ...state.remote,
            lastError: value,
          },
        })),
      setPairingSession: (value) =>
        set((state) => ({
          remote: {
            ...state.remote,
            pairingSession: value,
          },
        })),
      setRepos: (repos) =>
        set((state) => ({
          workbench: {
            ...state.workbench,
            repos,
            selectedRepoId:
              state.workbench.selectedRepoId &&
              repos.some((repo) => repo.repo_id === state.workbench.selectedRepoId)
                ? state.workbench.selectedRepoId
                : repos[0]?.repo_id ?? null,
          },
        })),
      setThreads: (repoId, threads) =>
        set((state) => ({
          workbench: {
            ...state.workbench,
            threadsByRepo: {
              ...state.workbench.threadsByRepo,
              [repoId]: threads,
            },
            selectedThreadIdByRepo: {
              ...state.workbench.selectedThreadIdByRepo,
              [repoId]: preferThreadSelection(
                state.workbench.selectedThreadIdByRepo[repoId],
                threads,
              ),
            },
          },
        })),
      upsertThread: (repoId, thread) =>
        set((state) => {
          const current = state.workbench.threadsByRepo[repoId] ?? [];
          const next = current.some((item) => item.local_thread_id === thread.local_thread_id)
            ? current.map((item) =>
                item.local_thread_id === thread.local_thread_id ? thread : item,
              )
            : [thread, ...current];

          return {
            workbench: {
              ...state.workbench,
              threadsByRepo: {
                ...state.workbench.threadsByRepo,
                [repoId]: next,
              },
              selectedThreadIdByRepo: {
                ...state.workbench.selectedThreadIdByRepo,
                [repoId]: thread.local_thread_id,
              },
            },
          };
        }),
      selectRepo: (repoId) =>
        set((state) => ({
          workbench: {
            ...state.workbench,
            selectedRepoId: repoId,
          },
        })),
      selectThread: (repoId, threadId) =>
        set((state) => ({
          workbench: {
            ...state.workbench,
            selectedThreadIdByRepo: {
              ...state.workbench.selectedThreadIdByRepo,
              [repoId]: threadId,
            },
          },
        })),
      updateThreadRun: (repoId, threadId, updater) =>
        set((state) => {
          const threads = state.workbench.threadsByRepo[repoId] ?? [];
          return {
            workbench: {
              ...state.workbench,
              threadsByRepo: {
                ...state.workbench.threadsByRepo,
                [repoId]: threads.map((thread) =>
                  thread.local_thread_id === threadId ? updater(thread) : thread,
                ),
              },
            },
          };
        }),
      clearRuntimeState: () =>
        set((state) => ({
          workbench: {
            ...initialWorkbenchState,
            selectedRepoId: state.workbench.selectedRepoId,
            selectedThreadIdByRepo: state.workbench.selectedThreadIdByRepo,
          },
        })),
      pushRuntimeLog: (level, message) =>
        set((state) => ({
          workbench: {
            ...state.workbench,
            runtimeLog: appendRuntimeLog(state.workbench.runtimeLog, level, message),
          },
        })),
      setHostRuntimeStatus: (value) =>
        set((state) => ({
          host: {
            ...state.host,
            runtimeStatus: value,
          },
        })),
      setHostLastError: (value) =>
        set((state) => ({
          host: {
            ...state.host,
            lastError: value,
          },
        })),
      setHostConfig: (value) =>
        set((state) => ({
          host: {
            ...state.host,
            config: {
              ...state.host.config,
              ...value,
            },
          },
        })),
      replaceHostLogs: (value) =>
        set((state) => ({
          host: {
            ...state.host,
            recentLogs: value,
          },
        })),
      pushHostLog: (level, message) =>
        set((state) => ({
          host: {
            ...state.host,
            recentLogs: appendRuntimeLog(state.host.recentLogs, level, message),
          },
        })),
    }),
    {
      name: "mycodex-desktop-session",
      partialize: (state) => ({
        mode: state.mode,
        remote: {
          serverUrl: state.remote.serverUrl,
          deviceLabel: state.remote.deviceLabel,
          bearerToken: state.remote.bearerToken,
          connectionStatus: initialRemoteState.connectionStatus,
          lastError: null,
          pairingSession: null,
        },
        host: {
          ...initialHostState,
          config: state.host.config,
        },
        workbench: {
          ...initialWorkbenchState,
          selectedRepoId: state.workbench.selectedRepoId,
          selectedThreadIdByRepo: state.workbench.selectedThreadIdByRepo,
        },
      }),
    },
  ),
);

export function selectedThreadForRepo(
  repoId: string | null,
  workbench: Pick<
    WorkbenchState,
    "threadsByRepo" | "selectedThreadIdByRepo"
  >,
): AppThreadSummary | null {
  if (!repoId) {
    return null;
  }

  const threads = workbench.threadsByRepo[repoId] ?? [];
  const selectedThreadId = workbench.selectedThreadIdByRepo[repoId];
  return (
    threads.find((thread) => thread.local_thread_id === selectedThreadId) ?? threads[0] ?? null
  );
}

export function withPendingRequest(
  thread: AppThreadSummary,
  pendingRequest: AppPendingRequest | null,
): AppThreadSummary {
  return {
    ...thread,
    active_run: thread.active_run
      ? {
          ...thread.active_run,
          pending_request: pendingRequest,
        }
      : pendingRequest
        ? {
            turn_id: pendingRequest.turn_id,
            assistant_text: "",
            command_output_tail: "",
            diff_preview: "",
            pending_request: pendingRequest,
          }
        : null,
  };
}
