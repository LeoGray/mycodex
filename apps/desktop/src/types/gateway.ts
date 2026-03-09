export type RpcId = number | string;

export type PairingPollStatus =
  | "pending"
  | "approved"
  | "claimed"
  | "rejected"
  | "expired";

export type AppRepoSummary = {
  repo_id: string;
  name: string;
  path: string;
  origin_url: string | null;
  app_thread_count: number;
};

export type CommandPendingRequest = {
  request_id: RpcId;
  kind: "command";
  thread_title: string;
  turn_id: string;
  item_id: string;
  command: string | null;
  cwd: string | null;
  reason: string | null;
};

export type FilePendingRequest = {
  request_id: RpcId;
  kind: "file";
  thread_title: string;
  turn_id: string;
  item_id: string;
  paths: string[];
  reason: string | null;
  diff_preview: string;
  preferred_decision: string;
};

export type AppPendingRequest = CommandPendingRequest | FilePendingRequest;

export type ThreadActiveRun = {
  turn_id: string;
  assistant_text: string;
  command_output_tail: string;
  diff_preview: string;
  pending_request: AppPendingRequest | null;
};

export type AppThreadSummary = {
  local_thread_id: string;
  codex_thread_id: string;
  title: string;
  status: string;
  created_at: string;
  last_used_at: string;
  has_user_message: boolean;
  active_run: ThreadActiveRun | null;
};

export type PairingRequestResponse = {
  pairing_id: string;
  pairing_code: string;
  expires_at: string;
};

export type PairingPollResponse = {
  pairing_id: string;
  status: PairingPollStatus;
  expires_at: string;
  device_id?: string | null;
  device_label?: string | null;
  token?: string | null;
};

export type ReposListResponse = {
  repos: AppRepoSummary[];
};

export type ThreadsListResponse = {
  repo_id: string;
  threads: AppThreadSummary[];
};

export type ThreadMutationResponse = {
  repo_id?: string;
  thread_id?: string;
  turn_id?: string;
  status?: string;
};

export type RunStartedEvent = {
  repo_id: string;
  thread_id: string;
  turn_id: string;
};

export type RunDeltaEvent = {
  repo_id: string;
  thread_id: string;
  turn_id: string;
  delta: string;
  assistant_text: string;
};

export type RunCommandOutputEvent = {
  repo_id: string;
  thread_id: string;
  turn_id: string;
  delta: string;
  command_output_tail: string;
};

export type RunDiffEvent = {
  repo_id: string;
  thread_id: string;
  turn_id: string;
  diff_preview: string;
};

export type RunCompletedEvent = {
  repo_id: string;
  thread_id: string;
  turn_id: string;
  status: string;
  assistant_text: string;
  command_output_tail: string;
  diff_preview: string;
  error?: string | null;
};

export type RunFailedEvent = RunCompletedEvent;

export type RunApprovalRequiredEvent = {
  repo_id: string;
  thread_id: string;
  turn_id: string;
  request_id: RpcId;
  kind: "command" | "file";
  thread_title: string;
  command?: string | null;
  cwd?: string | null;
  reason?: string | null;
  paths?: string[];
  diff_preview?: string;
};

export type GatewayNotification =
  | { method: "run.started"; params: RunStartedEvent }
  | { method: "run.delta"; params: RunDeltaEvent }
  | { method: "run.command_output"; params: RunCommandOutputEvent }
  | { method: "run.diff"; params: RunDiffEvent }
  | { method: "run.approval_required"; params: RunApprovalRequiredEvent }
  | { method: "run.completed"; params: RunCompletedEvent }
  | { method: "run.failed"; params: RunFailedEvent };
