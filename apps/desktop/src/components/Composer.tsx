type ComposerProps = {
  value: string;
  disabled: boolean;
  busy: boolean;
  canAbort: boolean;
  compact: boolean;
  onChange: (value: string) => void;
  onSend: () => void;
  onAbort: () => void;
};

export function Composer({
  value,
  disabled,
  busy,
  canAbort,
  compact,
  onChange,
  onSend,
  onAbort,
}: ComposerProps) {
  return (
    <div className="composer composer-card">
      <textarea
        rows={compact ? 5 : 4}
        value={value}
        onChange={(event) => onChange(event.target.value)}
        onKeyDown={(event) => {
          if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
            event.preventDefault();
            onSend();
          }
        }}
        placeholder="Send a task into the current thread..."
      />
      <p className="hint-inline">
        Use <code>Cmd/Ctrl + Enter</code> on desktop, or tap <strong>Send</strong> on
        mobile.
      </p>
      <div className="card-actions">
        <button disabled={disabled || busy} onClick={onSend}>
          Send
        </button>
        <button className="ghost" disabled={!canAbort} onClick={onAbort}>
          Abort run
        </button>
      </div>
    </div>
  );
}
