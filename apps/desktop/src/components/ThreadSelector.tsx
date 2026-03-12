import type { AppThreadSummary } from "../types/gateway";

type ThreadSelectorProps = {
  threads: AppThreadSummary[];
  selectedThreadId: string | null;
  disabled?: boolean;
  onSelect: (threadId: string) => void;
};

export function ThreadSelector({
  threads,
  selectedThreadId,
  disabled,
  onSelect,
}: ThreadSelectorProps) {
  return (
    <label className="toolbar-field">
      <span>Thread</span>
      <select
        value={selectedThreadId ?? ""}
        disabled={disabled || threads.length === 0}
        onChange={(event) => onSelect(event.target.value)}
      >
        {threads.length === 0 ? <option value="">No threads loaded</option> : null}
        {threads.map((thread) => (
          <option key={thread.local_thread_id} value={thread.local_thread_id}>
            {thread.title}
          </option>
        ))}
      </select>
    </label>
  );
}
