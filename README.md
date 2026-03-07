# MyCodex

**Language:** English | [简体中文](./README.zh-CN.md)

`MyCodex` is a Telegram-driven remote coding gateway for Linux servers.

It manages multiple Git repositories inside one workspace and uses the locally installed `codex` CLI to provide isolated Codex runtimes and multiple conversation threads per repository.

## Goals

- Run on Linux servers.
- Control Codex remotely through Telegram direct messages.
- Support multiple first-level repositories inside one workspace.
- Keep each repository isolated at the Codex runtime boundary.
- Allow multiple Codex threads per repository.
- Let the user switch repositories, switch threads, clone repositories, create new threads, and approve commands or patches from Telegram.

## Core Model

Internally, the system is built around three layers:

- `workspace`
  - A container directory for repositories, such as `/srv/workspace`
  - Only first-level child directories are scanned
- `repo`
  - The real working unit
  - Each repo has its own Codex runtime boundary
  - Switching repos does not reuse another repo's runtime
- `thread`
  - A Codex conversation inside a repo
  - One repo can have multiple threads
  - One repo has exactly one active thread at a time

This means:

- Repo A and repo B do not share the same Codex process context
- Repo A can keep multiple historical threads
- When you switch back to repo A, you resume repo A's active thread instead of inheriting repo B's context

## What Is Implemented

Current MVP capabilities:

- Telegram long polling
- Single-user allowlist
- Workspace first-level repository scan
- Clone new repositories into the workspace
- `/repo use` to switch the active repo
- `/thread new` and `/thread use` inside each repo
- Plain text messages routed to the active thread of the active repo
- Codex command approvals
- Codex file change approvals
- Local state persistence

## Current Limits

This is still an MVP. Current constraints:

- Telegram only
- Single user only
- Only scans first-level directories under the workspace
- No nested repo, submodule, or git worktree support
- `clone` only uses the default branch
- Does not manage Codex login flows
- No web UI
- No multi-user isolation

## Prerequisites

Before deployment, prepare:

1. A Linux server
2. A working `codex` CLI installation
3. Valid Codex authentication on that machine
4. A Telegram bot token
5. A working `git` setup on the server
6. A workspace directory that will contain multiple repos

Recommended sanity checks:

```bash
codex --version
codex app-server --help
git --version
```

## Codex Authentication

`MyCodex` does not manage Codex login. It only uses whatever authentication is already available on the server.

The recommended option is an environment variable:

```bash
export OPENAI_API_KEY=...
```

If your server already has a working `codex` login state, that can be reused as well. The `check` command validates:

- Telegram token availability
- `codex app-server` initialization
- Workspace existence
- Repository scanning inside the workspace

## Configuration

Start from the example:

[config/config.example.toml](./config/config.example.toml)

Example:

```toml
[workspace]
root = "/srv/workspace"

[telegram]
bot_token = "123456:replace-me"
allowed_user_id = 123456789
allowed_chat_id = 123456789
poll_timeout_seconds = 30

[codex]
bin = "codex"
# model = "gpt-5.1-codex"

[state]
dir = "/var/lib/mycodex"

[ui]
stream_edit_interval_ms = 1200
max_inline_diff_chars = 6000

[git]
clone_timeout_sec = 600
allow_ssh = true
allow_https = true
```

### Important Settings

- `workspace.root`
  - Repository container directory
  - Example: `/srv/workspace`
- `telegram.bot_token`
  - Telegram bot token
- `telegram.allowed_user_id`
  - Telegram user ID allowed to control the bot
- `telegram.allowed_chat_id`
  - Optional extra chat restriction
- `codex.bin`
  - Path or command name for the `codex` executable
- `codex.model`
  - Optional explicit model override for turns
- `state.dir`
  - MyCodex state directory
  - Stores repo/thread mappings, active repo/thread, temporary patch files, and related state
- `ui.stream_edit_interval_ms`
  - Telegram streaming edit throttle interval
- `ui.max_inline_diff_chars`
  - Diffs above this size are sent as `.patch` files instead of inline text
- `git.clone_timeout_sec`
  - Timeout for `git clone`
- `git.allow_ssh`
  - Whether SSH URLs are allowed
- `git.allow_https`
  - Whether HTTPS URLs are allowed

## Local Run

For local development:

```bash
cargo build --release
./target/release/mycodex check --config ./config/config.example.toml
./target/release/mycodex serve --config ./config/config.example.toml
```

`check` performs startup validation. `serve` starts the Telegram polling loop and Codex event loop.

## Install Entry Points

There are two separate installers in this repository:

- [scripts/install.sh](./scripts/install.sh)
  - For users who already cloned the repository
  - Builds from the local source tree and installs MyCodex
- [public/install.sh](./public/install.sh)
  - For a website-hosted one-line installer
  - Downloads a prebuilt release binary and installs MyCodex

These two entry points are intentionally separate.

## Scenario 1: Install From Source

Use this when:

- You already cloned the repository
- You want to build from the current source tree
- You are developing, testing, or doing a manual deployment

Example:

```bash
./scripts/install.sh \
  --telegram-bot-token 123456:replace-me \
  --telegram-user-id 123456789 \
  --openai-api-key sk-...
```

This mode will:

- Run `cargo build --release` from the current source tree
- Install the release binary
- Generate config and environment files
- Generate the systemd unit
- Run `mycodex check`
- Start the service by default

## Scenario 2: Install From a Prebuilt Release

Use this when:

- Users do not want to clone the repository
- You want a single install command
- You publish release binaries to GitHub Releases or another download URL

Recommended one-line install:

```bash
curl -fsSL https://raw.githubusercontent.com/LeoGray/mycodex/main/public/install.sh | bash
```

When run in a real terminal, the installer now prompts for:

- Telegram bot token
- Telegram user ID
- Optional OpenAI API key

It defaults to the GitHub repo `LeoGray/mycodex` and the latest release.

Advanced non-interactive example:

```bash
curl -fsSL https://raw.githubusercontent.com/LeoGray/mycodex/main/public/install.sh | bash -s -- \
  --github-repo LeoGray/mycodex \
  --release-version latest \
  --telegram-bot-token 123456:replace-me \
  --telegram-user-id 123456789
```

You can also provide a full asset URL:

```bash
curl -fsSL https://raw.githubusercontent.com/LeoGray/mycodex/main/public/install.sh | bash -s -- \
  --asset-url https://example.com/mycodex-x86_64-unknown-linux-gnu.tar.gz \
  --telegram-bot-token 123456:replace-me \
  --telegram-user-id 123456789
```

This mode will:

- Infer the Linux target triple from the current machine
- Download the matching release archive
- Extract the `mycodex` binary
- Continue with the same install flow used by the source installer

### OpenClaw-Style One-Liner

For this open-source setup, you can use GitHub Raw directly:

```bash
curl -fsSL https://raw.githubusercontent.com/LeoGray/mycodex/main/public/install.sh | bash
```

The public installer already defaults to `LeoGray/mycodex`. If you fork or rebrand the project, update the default GitHub repo near the top of `public/install.sh`.

### Default Installer Behavior

Both installers default to:

- Running the service as the current user instead of forcing a dedicated system user
- Reusing the current user's existing:
  - Codex auth state
  - Git credentials
  - SSH keys
- Using these default paths:
  - Binary: `/usr/local/bin/mycodex`
  - Config: `/etc/mycodex/config.toml`
  - Environment file: `/etc/mycodex/mycodex.env`
  - Service: `/etc/systemd/system/mycodex.service`
  - Workspace: `/srv/workspace`
  - State dir: `/var/lib/mycodex`

### Common Parameters For `scripts/install.sh`

The source installer supports:

- `--telegram-chat-id`
- `--run-user`
- `--run-group`
- `--workspace-root`
- `--state-dir`
- `--install-bin`
- `--config-path`
- `--env-path`
- `--service-path`
- `--codex-bin`
- `--codex-model`
- `--skip-build`
- `--disable-ssh`
- `--disable-https`
- `--no-start`

### Common Parameters For `public/install.sh`

The public installer supports:

- `--github-repo`
- `--release-version`
- `--asset-url`
- `--target-triple`
- `--telegram-chat-id`
- `--run-user`
- `--run-group`
- `--workspace-root`
- `--state-dir`
- `--install-bin`
- `--config-path`
- `--env-path`
- `--service-path`
- `--codex-bin`
- `--codex-model`
- `--disable-ssh`
- `--disable-https`
- `--no-start`

### Environment Variable Fallbacks

Supported installer environment variables:

- `MYCODEX_TELEGRAM_BOT_TOKEN`
- `MYCODEX_TELEGRAM_USER_ID`
- `MYCODEX_TELEGRAM_CHAT_ID`
- `MYCODEX_CODEX_BIN`
- `MYCODEX_CODEX_MODEL`
- `MYCODEX_RELEASE_GITHUB_REPO`
- `MYCODEX_RELEASE_VERSION`
- `MYCODEX_RELEASE_ASSET_URL`
- `MYCODEX_RELEASE_TARGET_TRIPLE`
- `OPENAI_API_KEY`

## Release Packaging

To support prebuilt release installs, the repo also includes:

[scripts/package-release.sh](./scripts/package-release.sh)

Examples:

```bash
./scripts/package-release.sh --target x86_64-unknown-linux-gnu
./scripts/package-release.sh --target aarch64-unknown-linux-musl
```

This generates archives such as:

- `dist/mycodex-x86_64-unknown-linux-gnu.tar.gz`
- `dist/mycodex-aarch64-unknown-linux-musl.tar.gz`

The release installer resolves assets using this naming convention by default.

## GitHub Actions Build And Release

The repository now includes two GitHub Actions workflows:

- [ci.yml](./.github/workflows/ci.yml)
  - Runs `cargo check` and `cargo test` on GitHub-hosted `ubuntu-latest`
- [release.yml](./.github/workflows/release.yml)
  - Builds Linux release artifacts when you push a tag
  - Currently produces:
    - `mycodex-x86_64-unknown-linux-gnu.tar.gz`
    - `mycodex-x86_64-unknown-linux-musl.tar.gz`
  - Uploads them to GitHub Releases

This is the missing piece that makes the public installer practical.

## systemd Deployment

Use this template as a starting point:

[deploy/systemd/mycodex.service](./deploy/systemd/mycodex.service)

Suggested directory layout:

- `/usr/local/bin/mycodex`
- `/etc/mycodex/config.toml`
- `/etc/mycodex/mycodex.env`
- `/var/lib/mycodex`
- `/srv/workspace`

### 1. Copy Binary And Config

```bash
sudo mkdir -p /etc/mycodex
sudo mkdir -p /var/lib/mycodex
sudo mkdir -p /srv/workspace
sudo cp ./target/release/mycodex /usr/local/bin/mycodex
sudo cp ./config/config.example.toml /etc/mycodex/config.toml
```

### 2. Write Environment File

Example:

```bash
sudo tee /etc/mycodex/mycodex.env >/dev/null <<'EOF'
OPENAI_API_KEY=replace-me
EOF
```

### 3. Install systemd Service

```bash
sudo cp ./deploy/systemd/mycodex.service /etc/systemd/system/mycodex.service
sudo systemctl daemon-reload
sudo systemctl enable --now mycodex
```

### 4. View Logs

```bash
sudo journalctl -u mycodex -f
```

## Telegram Commands

### Basic Commands

- `/start`
  - Show help
- `/status`
  - Show the active repo, active thread, runtime status, active turn, and pending approval
- `/abort`
  - Interrupt the active turn

### Repo Commands

- `/repo list`
  - List registered repos
- `/repo use <name>`
  - Switch the active repo
- `/repo clone <git_url> [dir_name]`
  - Clone a new repo into the workspace
- `/repo status`
  - Show the current repo state
- `/repo rescan`
  - Rescan existing repos under the workspace

### Thread Commands

- `/thread list`
  - List threads inside the current repo
- `/thread new`
  - Create a new thread in the current repo
- `/thread use <thread>`
  - Switch to an older thread in the current repo
  - `<thread>` can be a list index or a local thread ID prefix
- `/thread status`
  - Show the active thread in the current repo

### Plain Text Messages

Plain text is always routed to:

- The current active repo
- The current active thread

If the current repo does not yet have a thread, the first plain text message creates one automatically.

## Typical Flow

### Scenario 1: First Time In Repo A

```text
/repo rescan
/repo use repo-a
Fix the CI failure in this repository
```

What happens:

- MyCodex starts the Codex runtime for repo A
- If repo A has no thread yet, one is created automatically
- Further text messages go to repo A's active thread

### Scenario 2: Repo A Context Is Too Long

```text
/thread new
Let's rethink the implementation from scratch
```

What happens:

- You stay inside repo A
- No second repo runtime is started
- A new Codex thread is created under repo A

### Scenario 3: Switch To Repo B

```text
/repo use repo-b
Review the deployment scripts in this repository
```

What happens:

- Repo A's runtime is stopped
- Repo B's runtime is started
- Repo B uses its own thread set
- Repo A context is not reused

## Approval Behavior

When Codex wants to run a higher-risk command or apply file changes, MyCodex sends an approval message in Telegram.

### Command Approval

The message includes:

- Repo name
- Thread title
- CWD
- Command
- Reason

Buttons:

- `Approve once`
- `Decline`
- `Abort turn`

### File Approval

The message includes:

- Repo name
- Thread title
- Changed paths
- Diff preview

If the diff is too large, a `.patch` file is sent as well.

Buttons:

- `Approve patch`
- `Decline patch`

## State Persistence

MyCodex stores the following under `state.dir`:

- Repo catalog
- Thread catalog for each repo
- Current active repo
- Current active thread
- Current pending approval
- Current progress message ID

After a service restart:

- Repo and thread mappings are preserved
- The active repo is restored when possible
- Stale active turns are not restored
- Stale pending approvals are not restored

## Workspace Scan Rules

On startup and during `/repo rescan`, MyCodex only scans first-level directories under `workspace.root`.

Example:

```text
/srv/workspace
├── repo-a
├── repo-b
└── repo-c
```

Only directories like `repo-a`, `repo-b`, and `repo-c` are recognized.

## Clone Rules

Current clone behavior:

- Supports `https://...`
- Supports `ssh://...`
- Supports scp-style SSH URLs such as `git@host:org/repo.git`
- Uses the default branch
- Uses the repo basename as the default target directory
- Rejects the operation if the target directory already exists
- Relies on the server's existing SSH keys or credential helper for Git auth

## Troubleshooting

### `mycodex check` Fails

Check:

- Whether the Telegram token is correct
- Whether `codex` is on `PATH`
- Whether `codex app-server` can start
- Whether `OPENAI_API_KEY` or existing login state is valid
- Whether the workspace and state directories exist and are writable

### Telegram Does Not Respond

Check:

- Whether the bot token is correct
- Whether `allowed_user_id` is correct
- Whether `allowed_chat_id` matches the real chat
- Whether the logs show `telegram polling failed`

### Clone Fails

Check:

- Whether the server can access the remote Git host
- Whether SSH keys or credentials are valid
- Whether the target directory already exists
- Whether the URL is blocked by `git.allow_ssh` or `git.allow_https`

### Codex Runtime Problems

Check:

- `codex --version`
- `codex app-server --help`
- `OPENAI_API_KEY`
- `journalctl -u mycodex -f`
