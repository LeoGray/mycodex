import type { AppMode } from "../store/session";

type ModePickerProps = {
  allowLocalModes: boolean;
  onSelect: (mode: AppMode) => void;
};

export function ModePicker({ allowLocalModes, onSelect }: ModePickerProps) {
  return (
    <section className="mode-picker">
      <p className="eyebrow">MyCodex App</p>
      <h1>Choose how this app should work.</h1>
      <p className="lede">
        You can connect to an existing server, run this Mac as a local host, or do both
        at the same time.
      </p>
      <div className="mode-picker-grid">
        <button
          type="button"
          className="mode-option"
          onClick={() => onSelect("remote_client")}
        >
          <strong>Remote Client</strong>
          <span>Connect to an existing MyCodex server.</span>
        </button>
        {allowLocalModes ? (
          <>
            <button
              type="button"
              className="mode-option"
              onClick={() => onSelect("local_host")}
            >
              <strong>Local Host</strong>
              <span>Start a local server on this Mac without auto-connecting.</span>
            </button>
            <button
              type="button"
              className="mode-option"
              onClick={() => onSelect("local_host_client")}
            >
              <strong>Local Host + Client</strong>
              <span>Start a local server and connect this app to it automatically.</span>
            </button>
          </>
        ) : (
          <div className="mode-option mode-option-static">
            <strong>Mobile Client</strong>
            <span>Local Host modes are only available inside the desktop app shell.</span>
          </div>
        )}
      </div>
    </section>
  );
}
