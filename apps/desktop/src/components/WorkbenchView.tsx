import { Composer } from "./Composer";
import { RepoSelector } from "./RepoSelector";
import { ThreadSelector } from "./ThreadSelector";
import type { ConnectionStatus, RuntimeLog } from "../store/session";
import type { AppPendingRequest, AppRepoSummary, AppThreadSummary } from "../types/gateway";

type WorkbenchViewProps = {
  compact: boolean;
  workspaceLabel: string;
  connectionStatus: ConnectionStatus;
  repos: AppRepoSummary[];
  repoThreads: AppThreadSummary[];
  selectedRepoId: string | null;
  selectedRepoName: string | null;
  selectedThread: AppThreadSummary | null;
  runtimeLog: RuntimeLog[];
  threadTitleInput: string;
  messageInput: string;
  busyAction: string | null;
  onSelectRepo: (repoId: string) => void;
  onSelectThread: (threadId: string) => void;
  onThreadTitleInputChange: (value: string) => void;
  onMessageInputChange: (value: string) => void;
  onCreateThread: () => void;
  onRefreshThreads: () => void;
  onSendMessage: () => void;
  onAbortRun: () => void;
  onRespondApproval: (decision: "accept" | "decline" | "cancel") => void;
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

function shortId(value: string | null | undefined): string {
  return value ? value.slice(0, 8) : "n/a";
}

function describePendingRequest(pendingRequest: AppPendingRequest): string {
  if (pendingRequest.kind === "command") {
    return pendingRequest.command || "Command approval";
  }
  return pendingRequest.paths.join(", ") || "File approval";
}

export function WorkbenchView({
  compact,
  workspaceLabel,
  connectionStatus,
  repos,
  repoThreads,
  selectedRepoId,
  selectedRepoName,
  selectedThread,
  runtimeLog,
  threadTitleInput,
  messageInput,
  busyAction,
  onSelectRepo,
  onSelectThread,
  onThreadTitleInputChange,
  onMessageInputChange,
  onCreateThread,
  onRefreshThreads,
  onSendMessage,
  onAbortRun,
  onRespondApproval,
}: WorkbenchViewProps) {
  return (
    <section className="page-shell workbench-shell">
      <header className="workbench-toolbar">
        <div className="workspace-summary">
          <span className="eyebrow">Workspace</span>
          <strong>{workspaceLabel}</strong>
        </div>
        <div className="toolbar-pickers">
          <RepoSelector
            repos={repos}
            selectedRepoId={selectedRepoId}
            disabled={connectionStatus !== "connected"}
            onSelect={onSelectRepo}
          />
          <ThreadSelector
            threads={repoThreads}
            selectedThreadId={selectedThread?.local_thread_id ?? null}
            disabled={connectionStatus !== "connected"}
            onSelect={onSelectThread}
          />
        </div>
      </header>

      <div className="workbench-grid">
        <section className="surface-card control-card">
          <div className="card-header">
            <h2>Thread Control</h2>
            <span>{selectedRepoName ?? "Select a repo"}</span>
          </div>
          <label>
            <span>New thread title</span>
            <input
              value={threadTitleInput}
              onChange={(event) => onThreadTitleInputChange(event.target.value)}
              placeholder="Optional title"
            />
          </label>
          <div className="card-actions">
            <button
              disabled={
                !selectedRepoId ||
                busyAction === "thread" ||
                connectionStatus !== "connected"
              }
              onClick={onCreateThread}
            >
              New thread
            </button>
            <button
              className="ghost"
              disabled={!selectedRepoId || connectionStatus !== "connected"}
              onClick={onRefreshThreads}
            >
              Refresh
            </button>
          </div>
          {selectedThread ? (
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
                <span>
                  {selectedThread.active_run
                    ? shortId(selectedThread.active_run.turn_id)
                    : "idle"}
                </span>
              </div>
            </div>
          ) : (
            <p className="empty">Pick a repo and thread to start working.</p>
          )}
        </section>

        <section className="surface-card output-card">
          <div className="card-header">
            <h2>Workbench</h2>
            <span>{selectedThread?.title ?? "No thread selected"}</span>
          </div>
          {selectedThread ? (
            <>
              <div className="output-grid">
                <section className="output-panel">
                  <h3>Assistant</h3>
                  <pre>{selectedThread.active_run?.assistant_text || "No output yet."}</pre>
                </section>
                <section className="output-panel">
                  <h3>Command output</h3>
                  <pre>
                    {selectedThread.active_run?.command_output_tail ||
                      "No command output yet."}
                  </pre>
                </section>
                <section className="output-panel full">
                  <h3>Diff preview</h3>
                  <pre>{selectedThread.active_run?.diff_preview || "No diff yet."}</pre>
                </section>
              </div>

              {selectedThread.active_run?.pending_request ? (
                <div className="approval-panel">
                  <div className="card-header">
                    <h3>Approval</h3>
                    <span>{selectedThread.active_run.pending_request.kind}</span>
                  </div>
                  <div className="approval-summary">
                    <p>
                      <strong>Target</strong>
                      <span>
                        {describePendingRequest(selectedThread.active_run.pending_request)}
                      </span>
                    </p>
                    <p>
                      <strong>Reason</strong>
                      <span>
                        {selectedThread.active_run.pending_request.reason || "none"}
                      </span>
                    </p>
                  </div>
                  {selectedThread.active_run.pending_request.kind === "file" ? (
                    <pre className="approval-diff">
                      {selectedThread.active_run.pending_request.diff_preview ||
                        "No diff preview."}
                    </pre>
                  ) : null}
                  <div className="card-actions">
                    <button
                      disabled={busyAction === "approval"}
                      onClick={() => onRespondApproval("accept")}
                    >
                      Accept
                    </button>
                    <button
                      className="ghost"
                      disabled={busyAction === "approval"}
                      onClick={() => onRespondApproval("decline")}
                    >
                      Decline
                    </button>
                    {selectedThread.active_run.pending_request.kind === "file" ? (
                      <button
                        className="ghost"
                        disabled={busyAction === "approval"}
                        onClick={() => onRespondApproval("cancel")}
                      >
                        Cancel
                      </button>
                    ) : null}
                  </div>
                </div>
              ) : null}
            </>
          ) : (
            <p className="empty">Select a repo and thread to load the workbench.</p>
          )}
        </section>

        <section className="surface-card activity-card">
          <div className="card-header">
            <h2>Activity</h2>
            <span>{runtimeLog.length} events</span>
          </div>
          <div className="log">
            {runtimeLog.length === 0 ? (
              <p className="empty">Activity will appear here once a run starts.</p>
            ) : (
              runtimeLog.map((entry) => (
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

      <Composer
        compact={compact}
        value={messageInput}
        disabled={
          connectionStatus !== "connected" || !selectedRepoId || !messageInput.trim()
        }
        busy={busyAction === "send"}
        canAbort={Boolean(selectedThread?.active_run?.turn_id) && busyAction !== "abort"}
        onChange={onMessageInputChange}
        onSend={onSendMessage}
        onAbort={onAbortRun}
      />
    </section>
  );
}
