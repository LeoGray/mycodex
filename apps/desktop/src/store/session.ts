import { create } from "zustand";
import { persist } from "zustand/middleware";

import type {
  AppPendingRequest,
  AppRepoSummary,
  AppThreadSummary,
  PairingPollStatus,
} from "../types/gateway";

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

type RuntimeLog = {
  id: string;
  level: "info" | "error";
  message: string;
  createdAt: string;
};

type SessionState = {
  serverUrl: string;
  deviceLabel: string;
  bearerToken: string;
  connectionStatus: ConnectionStatus;
  lastError: string | null;
  pairingSession: PairingSession | null;
  repos: AppRepoSummary[];
  threadsByRepo: Record<string, AppThreadSummary[]>;
  selectedRepoId: string | null;
  selectedThreadIdByRepo: Record<string, string | null>;
  runtimeLog: RuntimeLog[];
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

export const useSessionStore = create<SessionState>()(
  persist(
    (set, get) => ({
      serverUrl: "http://127.0.0.1:3940",
      deviceLabel: "MyCodex Desktop",
      bearerToken: "",
      connectionStatus: "idle",
      lastError: null,
      pairingSession: null,
      repos: [],
      threadsByRepo: {},
      selectedRepoId: null,
      selectedThreadIdByRepo: {},
      runtimeLog: [],
      setServerUrl: (value) => set({ serverUrl: value }),
      setDeviceLabel: (value) => set({ deviceLabel: value }),
      setBearerToken: (value) => set({ bearerToken: value }),
      setConnectionStatus: (value) => set({ connectionStatus: value }),
      setLastError: (value) => set({ lastError: value }),
      setPairingSession: (value) => set({ pairingSession: value }),
      setRepos: (repos) =>
        set((state) => ({
          repos,
          selectedRepoId:
            state.selectedRepoId && repos.some((repo) => repo.repo_id === state.selectedRepoId)
              ? state.selectedRepoId
              : repos[0]?.repo_id ?? null,
        })),
      setThreads: (repoId, threads) =>
        set((state) => ({
          threadsByRepo: {
            ...state.threadsByRepo,
            [repoId]: threads,
          },
          selectedThreadIdByRepo: {
            ...state.selectedThreadIdByRepo,
            [repoId]: preferThreadSelection(state.selectedThreadIdByRepo[repoId], threads),
          },
        })),
      upsertThread: (repoId, thread) =>
        set((state) => {
          const current = state.threadsByRepo[repoId] ?? [];
          const next = current.some((item) => item.local_thread_id === thread.local_thread_id)
            ? current.map((item) =>
                item.local_thread_id === thread.local_thread_id ? thread : item,
              )
            : [thread, ...current];

          return {
            threadsByRepo: {
              ...state.threadsByRepo,
              [repoId]: next,
            },
            selectedThreadIdByRepo: {
              ...state.selectedThreadIdByRepo,
              [repoId]: thread.local_thread_id,
            },
          };
        }),
      selectRepo: (repoId) => set({ selectedRepoId: repoId }),
      selectThread: (repoId, threadId) =>
        set((state) => ({
          selectedThreadIdByRepo: {
            ...state.selectedThreadIdByRepo,
            [repoId]: threadId,
          },
        })),
      updateThreadRun: (repoId, threadId, updater) =>
        set((state) => {
          const threads = state.threadsByRepo[repoId] ?? [];
          return {
            threadsByRepo: {
              ...state.threadsByRepo,
              [repoId]: threads.map((thread) =>
                thread.local_thread_id === threadId ? updater(thread) : thread,
              ),
            },
          };
        }),
      clearRuntimeState: () =>
        set((state) => ({
          repos: [],
          threadsByRepo: {},
          selectedRepoId: state.selectedRepoId,
          selectedThreadIdByRepo: state.selectedThreadIdByRepo,
          runtimeLog: [],
        })),
      pushRuntimeLog: (level, message) =>
        set((state) => ({
          runtimeLog: [
            {
              id: `${Date.now()}-${state.runtimeLog.length + 1}`,
              level,
              message,
              createdAt: new Date().toISOString(),
            },
            ...state.runtimeLog,
          ].slice(0, 24),
        })),
    }),
    {
      name: "mycodex-desktop-session",
      partialize: (state) => ({
        serverUrl: state.serverUrl,
        deviceLabel: state.deviceLabel,
        bearerToken: state.bearerToken,
        selectedRepoId: state.selectedRepoId,
        selectedThreadIdByRepo: state.selectedThreadIdByRepo,
      }),
    },
  ),
);

export function selectedThreadForRepo(
  repoId: string | null,
  threadsByRepo: Record<string, AppThreadSummary[]>,
  selectedThreadIdByRepo: Record<string, string | null>,
): AppThreadSummary | null {
  if (!repoId) {
    return null;
  }

  const threads = threadsByRepo[repoId] ?? [];
  const selectedThreadId = selectedThreadIdByRepo[repoId];
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
