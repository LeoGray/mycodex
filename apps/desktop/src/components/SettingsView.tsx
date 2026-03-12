import type { AppMode, ConnectionStatus, PairingSession } from "../store/session";

type SettingsViewProps = {
  mode: AppMode;
  allowLocalModes: boolean;
  connectionStatus: ConnectionStatus;
  serverUrl: string;
  deviceLabel: string;
  bearerToken: string;
  pairingSession: PairingSession | null;
  lastError: string | null;
  hostStatusLabel: string;
  selectedRepoName: string | null;
  selectedThreadTitle: string | null;
  busyAction: string | null;
  onServerUrlChange: (value: string) => void;
  onDeviceLabelChange: (value: string) => void;
  onBearerTokenChange: (value: string) => void;
  onModeChange: (value: AppMode) => void;
  onConnect: () => void;
  onDisconnect: () => void;
  onRefresh: () => void;
  onStartPairing: () => void;
  onPollPairing: () => void;
};

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

export function SettingsView({
  mode,
  allowLocalModes,
  connectionStatus,
  serverUrl,
  deviceLabel,
  bearerToken,
  pairingSession,
  lastError,
  hostStatusLabel,
  selectedRepoName,
  selectedThreadTitle,
  busyAction,
  onServerUrlChange,
  onDeviceLabelChange,
  onBearerTokenChange,
  onModeChange,
  onConnect,
  onDisconnect,
  onRefresh,
  onStartPairing,
  onPollPairing,
}: SettingsViewProps) {
  return (
    <section className="page-shell settings-shell">
      <div className="settings-grid">
        <section className="surface-card">
          <div className="card-header">
            <h2>Connection</h2>
            <span>{modeLabel(mode)}</span>
          </div>
          {allowLocalModes ? (
            <label>
              <span>Mode</span>
              <select
                value={mode}
                onChange={(event) => onModeChange(event.target.value as AppMode)}
              >
                <option value="remote_client">Remote Client</option>
                <option value="local_host">Local Host</option>
                <option value="local_host_client">Local Host + Client</option>
              </select>
            </label>
          ) : (
            <div className="pairing-box">
              <div>
                <strong>Mode</strong>
                <span>Remote Client</span>
              </div>
            </div>
          )}
          <label>
            <span>Server URL</span>
            <input
              value={serverUrl}
              onChange={(event) => onServerUrlChange(event.target.value)}
              placeholder="http://127.0.0.1:3940"
              autoCapitalize="none"
              autoCorrect="off"
              inputMode="url"
              spellCheck={false}
            />
          </label>
          <label>
            <span>Device label</span>
            <input
              value={deviceLabel}
              onChange={(event) => onDeviceLabelChange(event.target.value)}
              placeholder="MyCodex App"
            />
          </label>
          <label>
            <span>Bearer token</span>
            <input
              value={bearerToken}
              onChange={(event) => onBearerTokenChange(event.target.value)}
              placeholder="mcx_..."
              autoCapitalize="none"
              autoCorrect="off"
              spellCheck={false}
            />
          </label>
          <div className="card-actions">
            <button
              disabled={busyAction === "connect" || connectionStatus === "connected"}
              onClick={onConnect}
            >
              {connectionStatus === "connected" ? "Connected" : "Connect"}
            </button>
            <button className="ghost" onClick={onDisconnect}>
              Disconnect
            </button>
            <button
              className="ghost"
              disabled={connectionStatus !== "connected"}
              onClick={onRefresh}
            >
              Refresh
            </button>
          </div>
          {allowLocalModes ? (
            <p className="hint tight">
              Change how this app runs without leaving the current shell.
            </p>
          ) : null}
        </section>

        <section className="surface-card">
          <div className="card-header">
            <h2>Pairing</h2>
            <span>APP gateway</span>
          </div>
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
            <button disabled={busyAction === "pairing"} onClick={onStartPairing}>
              Start pairing
            </button>
            <button
              className="ghost"
              disabled={!pairingSession || busyAction === "pairing-poll"}
              onClick={onPollPairing}
            >
              Poll now
            </button>
          </div>
          <p className="hint">
            Approve with <code>mycodex app pairing approve &lt;CODE&gt;</code>.
          </p>
        </section>

        <section className="surface-card">
          <div className="card-header">
            <h2>Diagnostics</h2>
            <span>Current session</span>
          </div>
          <dl className="status-list">
            <div>
              <dt>Connection</dt>
              <dd>{connectionStatus}</dd>
            </div>
            <div>
              <dt>Host</dt>
              <dd>{hostStatusLabel}</dd>
            </div>
            <div>
              <dt>Repo</dt>
              <dd>{selectedRepoName ?? "none"}</dd>
            </div>
            <div>
              <dt>Thread</dt>
              <dd>{selectedThreadTitle ?? "none"}</dd>
            </div>
          </dl>
          {lastError ? <p className="error-text">{lastError}</p> : null}
        </section>
      </div>
    </section>
  );
}
