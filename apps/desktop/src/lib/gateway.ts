import type {
  AppPendingRequest,
  AppThreadSummary,
  GatewayNotification,
  PairingPollResponse,
  PairingRequestResponse,
  ReposListResponse,
  RpcId,
  ThreadMutationResponse,
  ThreadsListResponse,
} from "../types/gateway";

type JsonRpcSuccess<T> = {
  id: RpcId;
  result: T;
  error?: undefined;
};

type JsonRpcFailure = {
  id: RpcId;
  result?: undefined;
  error: {
    code: number;
    message: string;
  };
};

type JsonRpcEnvelope<T> = JsonRpcSuccess<T> | JsonRpcFailure;

type NotificationListener = (notification: GatewayNotification) => void;
type DisconnectListener = (message: string) => void;

function withProtocol(raw: string): string {
  const value = raw.trim();
  if (!value) {
    return "";
  }
  if (value.startsWith("http://") || value.startsWith("https://")) {
    return value;
  }
  return `http://${value}`;
}

function normalizeBaseUrl(raw: string): string {
  const value = withProtocol(raw);
  const url = new URL(value);
  url.pathname = url.pathname.replace(/\/+$/, "");
  return url.toString().replace(/\/$/, "");
}

function buildHttpUrl(baseUrl: string, path: string): string {
  return new URL(path, `${normalizeBaseUrl(baseUrl)}/`).toString();
}

function buildWebSocketUrl(baseUrl: string, bearerToken: string): string {
  const url = new URL("/ws", `${normalizeBaseUrl(baseUrl)}/`);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  url.searchParams.set("token", bearerToken);
  return url.toString();
}

async function readJson<T>(response: Response): Promise<T> {
  if (!response.ok) {
    let message = `${response.status} ${response.statusText}`;
    try {
      const payload = (await response.json()) as { error?: string };
      if (payload.error) {
        message = payload.error;
      }
    } catch {
      // Ignore invalid JSON on error paths.
    }
    throw new Error(message);
  }
  return (await response.json()) as T;
}

export async function requestPairing(
  baseUrl: string,
  deviceLabel: string,
): Promise<PairingRequestResponse> {
  const response = await fetch(buildHttpUrl(baseUrl, "/api/app/pairings/request"), {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ device_label: deviceLabel }),
  });
  return readJson<PairingRequestResponse>(response);
}

export async function pollPairing(
  baseUrl: string,
  pairingId: string,
): Promise<PairingPollResponse> {
  const response = await fetch(
    buildHttpUrl(baseUrl, `/api/app/pairings/${encodeURIComponent(pairingId)}`),
  );
  return readJson<PairingPollResponse>(response);
}

export function approvalDecisionForPendingRequest(
  pendingRequest: AppPendingRequest,
  decision: "accept" | "decline" | "cancel",
): "accept" | "decline" | "cancel" {
  if (pendingRequest.kind === "file" && decision === "cancel") {
    return "cancel";
  }
  return decision;
}

export class GatewayRpcClient {
  private readonly baseUrl: string;
  private readonly bearerToken: string;
  private socket: WebSocket | null = null;
  private nextId = 1;
  private readonly pending = new Map<
    number,
    {
      resolve: (value: unknown) => void;
      reject: (error: Error) => void;
    }
  >();
  private readonly listeners = new Set<NotificationListener>();
  private readonly disconnectListeners = new Set<DisconnectListener>();

  constructor(baseUrl: string, bearerToken: string) {
    this.baseUrl = normalizeBaseUrl(baseUrl);
    this.bearerToken = bearerToken;
  }

  async connect(): Promise<void> {
    if (this.socket && this.socket.readyState === WebSocket.OPEN) {
      return;
    }

    await new Promise<void>((resolve, reject) => {
      const socket = new WebSocket(buildWebSocketUrl(this.baseUrl, this.bearerToken));
      this.socket = socket;

      socket.onopen = () => resolve();
      socket.onerror = () => reject(new Error("WebSocket connection failed"));
      socket.onclose = (event) => {
        this.rejectPending(new Error("APP gateway disconnected"));
        this.emitDisconnect(
          event.reason || `Connection closed${event.code ? ` (${event.code})` : ""}`,
        );
      };
      socket.onmessage = (event) => {
        this.handleMessage(event.data);
      };
    });
  }

  disconnect(): void {
    if (!this.socket) {
      return;
    }
    this.socket.onclose = null;
    this.socket.close();
    this.socket = null;
    this.rejectPending(new Error("APP gateway disconnected"));
  }

  onNotification(listener: NotificationListener): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  onDisconnect(listener: DisconnectListener): () => void {
    this.disconnectListeners.add(listener);
    return () => {
      this.disconnectListeners.delete(listener);
    };
  }

  async listRepos(): Promise<ReposListResponse> {
    return this.call<ReposListResponse>("repos.list", {});
  }

  async listThreads(repoId: string): Promise<ThreadsListResponse> {
    return this.call<ThreadsListResponse>("threads.list", { repo_id: repoId });
  }

  async createThread(repoId: string, title?: string): Promise<AppThreadSummary> {
    return this.call<AppThreadSummary>("threads.create", {
      repo_id: repoId,
      title,
    });
  }

  async sendToThread(
    repoId: string,
    threadId: string,
    text: string,
  ): Promise<ThreadMutationResponse> {
    return this.call<ThreadMutationResponse>("threads.send", {
      repo_id: repoId,
      thread_id: threadId,
      text,
    });
  }

  async abortRun(repoId: string, turnId: string): Promise<ThreadMutationResponse> {
    return this.call<ThreadMutationResponse>("runs.abort", {
      repo_id: repoId,
      turn_id: turnId,
    });
  }

  async respondApproval(
    repoId: string,
    requestId: RpcId,
    decision: "accept" | "decline" | "cancel",
  ): Promise<ThreadMutationResponse> {
    return this.call<ThreadMutationResponse>("approvals.respond", {
      repo_id: repoId,
      request_id: requestId,
      decision,
    });
  }

  private call<T>(method: string, params: Record<string, unknown>): Promise<T> {
    const socket = this.socket;
    if (!socket || socket.readyState !== WebSocket.OPEN) {
      return Promise.reject(new Error("APP gateway is not connected"));
    }

    const id = this.nextId++;
    const payload = JSON.stringify({
      id,
      method,
      params,
    });

    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, {
        resolve: (value) => resolve(value as T),
        reject,
      });

      try {
        socket.send(payload);
      } catch (error) {
        this.pending.delete(id);
        reject(
          error instanceof Error ? error : new Error("Failed to write WebSocket message"),
        );
      }
    });
  }

  private handleMessage(raw: unknown): void {
    if (typeof raw !== "string") {
      return;
    }

    let payload: JsonRpcEnvelope<unknown> | GatewayNotification;
    try {
      payload = JSON.parse(raw) as JsonRpcEnvelope<unknown> | GatewayNotification;
    } catch {
      this.emitDisconnect("Received invalid JSON from APP gateway");
      return;
    }

    if ("method" in payload) {
      for (const listener of this.listeners) {
        listener(payload);
      }
      return;
    }

    const numericId =
      typeof payload.id === "number" ? payload.id : Number.parseInt(String(payload.id), 10);
    if (!Number.isFinite(numericId)) {
      return;
    }

    const pending = this.pending.get(numericId);
    if (!pending) {
      return;
    }
    this.pending.delete(numericId);

    if ("error" in payload && payload.error) {
      pending.reject(new Error(payload.error.message));
      return;
    }
    pending.resolve(payload.result);
  }

  private rejectPending(error: Error): void {
    for (const entry of this.pending.values()) {
      entry.reject(error);
    }
    this.pending.clear();
  }

  private emitDisconnect(message: string): void {
    for (const listener of this.disconnectListeners) {
      listener(message);
    }
  }
}
