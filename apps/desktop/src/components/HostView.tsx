import type { RuntimeLog } from "../store/session";
import type { HostRuntimeStatus } from "../lib/host";

type HostDraft = {
  networkMode: "local_only" | "lan";
  port: number;
  workspaceRoot: string;
  stateDir: string;
  codexBin: string;
  binaryPath: string;
  workingDirectory: string;
};

type HostViewProps = {
  status: HostRuntimeStatus;
  lastError: string | null;
  hostDraft: HostDraft;
  bindAddress: string;
  lanAddress: string | null;
  configPath: string;
  logPath: string;
  telegramEnabled: boolean;
  recentLogs: RuntimeLog[];
  busyAction: string | null;
  onDraftChange: (draft: HostDraft) => void;
  onApplyConfig: () => void;
  onStartHost: () => void;
  onStopHost: () => void;
  onRestartHost: () => void;
  onRefreshHost: () => void;
};

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

export function HostView({
  status,
  lastError,
  hostDraft,
  bindAddress,
  lanAddress,
  configPath,
  logPath,
  telegramEnabled,
  recentLogs,
  busyAction,
  onDraftChange,
  onApplyConfig,
  onStartHost,
  onStopHost,
  onRestartHost,
  onRefreshHost,
}: HostViewProps) {
  const canStart = status === "stopped" || status === "crashed";
  const canStop = status === "running" || status === "starting";
  const canRestart = status === "running";

  return (
    <section className="page-shell host-shell">
      <div className="host-grid">
        <section className="surface-card host-runtime-card">
          <div className="card-header">
            <h2>Host Runtime</h2>
            <span>{status}</span>
          </div>
          <div className="pairing-box">
            <div>
              <strong>Bind address</strong>
              <span>{bindAddress}</span>
            </div>
            <div>
              <strong>LAN URL</strong>
              <span>{lanAddress ?? "local only"}</span>
            </div>
            <div>
              <strong>Telegram</strong>
              <span>{telegramEnabled ? "enabled" : "disabled"}</span>
            </div>
          </div>
          <div className="card-actions">
            <button disabled={busyAction === "host-start" || !canStart} onClick={onStartHost}>
              Start
            </button>
            <button className="ghost" disabled={!canStop} onClick={onStopHost}>
              Stop
            </button>
            <button className="ghost" disabled={busyAction === "host-restart" || !canRestart} onClick={onRestartHost}>
              Restart
            </button>
            <button className="ghost" onClick={onRefreshHost}>
              Refresh
            </button>
          </div>
          <p className="hint tight">
            {status === "running"
              ? "Local host is up. Use Restart after config changes, or Stop to take it down."
              : status === "starting"
                ? "Local host is starting. Refresh if this view does not update in a moment."
                : status === "crashed"
                  ? "Local host is down after a failed start. Check the latest log entry below."
                  : "Local host is not running yet."}
          </p>
          {lastError ? <p className="error-text">{lastError}</p> : null}
        </section>

        <section className="surface-card host-config-card">
          <div className="card-header">
            <h2>Host Config</h2>
            <span>App-managed</span>
          </div>
          <div className="host-config-grid">
            <label>
              <span>Network mode</span>
              <select
                value={hostDraft.networkMode}
                onChange={(event) =>
                  onDraftChange({
                    ...hostDraft,
                    networkMode: event.target.value as "local_only" | "lan",
                  })
                }
              >
                <option value="local_only">Local only</option>
                <option value="lan">Allow LAN devices</option>
              </select>
            </label>
            <label>
              <span>Port</span>
              <input
                type="number"
                min={1}
                value={hostDraft.port}
                onChange={(event) =>
                  onDraftChange({
                    ...hostDraft,
                    port: Number(event.target.value || 3940),
                  })
                }
              />
            </label>
            <label className="field-span-2">
              <span>Workspace root</span>
              <input
                value={hostDraft.workspaceRoot}
                onChange={(event) =>
                  onDraftChange({
                    ...hostDraft,
                    workspaceRoot: event.target.value,
                  })
                }
              />
            </label>
            <label>
              <span>State dir</span>
              <input
                value={hostDraft.stateDir}
                onChange={(event) =>
                  onDraftChange({
                    ...hostDraft,
                    stateDir: event.target.value,
                  })
                }
              />
            </label>
            <label>
              <span>Codex bin</span>
              <input
                value={hostDraft.codexBin}
                onChange={(event) =>
                  onDraftChange({
                    ...hostDraft,
                    codexBin: event.target.value,
                  })
                }
              />
            </label>
            <label className="field-span-2">
              <span>Binary override</span>
              <input
                value={hostDraft.binaryPath}
                onChange={(event) =>
                  onDraftChange({
                    ...hostDraft,
                    binaryPath: event.target.value,
                  })
                }
                placeholder="Optional"
              />
            </label>
            <label className="field-span-2">
              <span>Working directory</span>
              <input
                value={hostDraft.workingDirectory}
                onChange={(event) =>
                  onDraftChange({
                    ...hostDraft,
                    workingDirectory: event.target.value,
                  })
                }
                placeholder="Optional"
              />
            </label>
          </div>
          <div className="card-actions">
            <button onClick={onApplyConfig}>Apply config</button>
          </div>
          <p className="hint tight">
            Config file: <code>{configPath || "pending"}</code>
          </p>
          <p className="hint tight">
            Log file: <code>{logPath || "pending"}</code>
          </p>
        </section>

        <section className="surface-card host-logs-card">
          <div className="card-header">
            <h2>Host Logs</h2>
            <span>{recentLogs.length} lines</span>
          </div>
          <div className="log">
            {recentLogs.length === 0 ? (
              <p className="empty">Host logs will appear here after the server starts.</p>
            ) : (
              recentLogs.map((entry) => (
                <article key={entry.id} className={`log-entry log-${entry.level}`}>
                  <div className="log-heading">
                    <strong>{entry.level}</strong>
                    <span>{formatTimestamp(entry.createdAt)}</span>
                  </div>
                  <p>{entry.message}</p>
                </article>
              ))
            )}
          </div>
        </section>
      </div>
    </section>
  );
}
