import type { AppRepoSummary } from "../types/gateway";

type RepoSelectorProps = {
  repos: AppRepoSummary[];
  selectedRepoId: string | null;
  disabled?: boolean;
  onSelect: (repoId: string) => void;
};

export function RepoSelector({
  repos,
  selectedRepoId,
  disabled,
  onSelect,
}: RepoSelectorProps) {
  return (
    <label className="toolbar-field">
      <span>Repo</span>
      <select
        value={selectedRepoId ?? ""}
        disabled={disabled || repos.length === 0}
        onChange={(event) => onSelect(event.target.value)}
      >
        {repos.length === 0 ? <option value="">No repos loaded</option> : null}
        {repos.map((repo) => (
          <option key={repo.repo_id} value={repo.repo_id}>
            {repo.name}
          </option>
        ))}
      </select>
    </label>
  );
}
