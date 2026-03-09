# MyCodex

**Language:** English | [简体中文](./README.zh-CN.md)

MyCodex is a remote coding gateway for `codex`.
It lets you run Codex against multiple Git repositories inside one workspace, with isolated repo state, multiple threads per repo, and optional control surfaces for Telegram and the desktop APP.

Core points:

- Telegram remains a first-class control surface
- The desktop APP can pair over HTTP and WebSocket when enabled
- One workspace can hold many first-level repos
- Each repo has its own Codex runtime boundary
- Each repo can have multiple threads
- Telegram threads and APP threads are isolated from each other
- Command and patch approvals route back to the surface that started the run

## Quick Start

Prepare these first:

- a working `codex` CLI
- Codex auth or `OPENAI_API_KEY`
- `git`
- a Telegram bot token

Official one-line install for x86_64 Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/LeoGray/mycodex/main/public/install.sh | bash
/usr/local/bin/mycodex onboard
```

Source install for Linux or macOS:

```bash
git clone https://github.com/LeoGray/mycodex.git
cd mycodex
./scripts/install.sh --install-service
/usr/local/bin/mycodex onboard
```

`onboard` will:

- validate the Telegram bot token
- let you choose a workspace path
- optionally store `OPENAI_API_KEY`
- optionally enable the remote APP gateway
- optionally enable the installed service

## Repository Layout

- `apps/server`: Rust daemon, Telegram adapter, APP gateway, and CLI
- `apps/desktop`: Tauri + React desktop client shell
- `config`: example configuration
- `deploy`: service definitions
- `scripts`: install and packaging helpers

## Command Menu

Basic:

- `/start`
- `/status`
- `/abort`

Repo:

- `/repo list`
- `/repo use <name>`
- `/repo clone <git_url> [dir_name]`
- `/repo status`
- `/repo rescan`

Thread:

- `/thread list`
- `/thread new`
- `/thread use <thread>`
- `/thread status`

Plain text messages are always sent to the active thread of the active repo.

## How It Works

- `workspace`: a directory that contains first-level repos
- `repo`: the runtime isolation boundary
- `thread`: a Codex conversation inside one repo
- `surface`: the interaction entry point, either `telegram` or `app`

Switching repos does not reuse another repo's runtime context.
Telegram threads and APP threads do not appear in each other's thread lists.

Telegram access mode is `pairing` by default.
First-time Telegram flow:

1. Install MyCodex
2. Run `/usr/local/bin/mycodex onboard`
3. Send a message to the bot
4. Get a pairing code
5. Approve it on the server with `/usr/local/bin/mycodex pairing approve <CODE>`

Desktop APP flow:

1. Enable the APP gateway during `onboard`, or set `app.enabled = true`
2. Start the daemon
3. Open the desktop APP and request a pairing code
4. Approve it on the server with `/usr/local/bin/mycodex app pairing approve <CODE>`
5. Connect the APP with the issued bearer token

## Config

Start from [config/config.example.toml](./config/config.example.toml).

Most important keys:

- `workspace.root`
- `telegram.bot_token`
- `telegram.access_mode`
- `app.enabled`
- `app.bind_addr`
- `app.public_base_url`
- `codex.bin`
- `state.dir`

Default paths:

- Linux
  - config: `/etc/mycodex/config.toml`
  - env: `/etc/mycodex/mycodex.env`
  - service: `/etc/systemd/system/mycodex.service`
  - state: `/var/lib/mycodex`
- macOS
  - config: `$HOME/.config/mycodex/config.toml`
  - env: `$HOME/.config/mycodex/mycodex.env`
  - service: `$HOME/Library/LaunchAgents/com.leogray.mycodex.plist`
  - state: `$HOME/.local/state/mycodex`

## Install And Release Notes

- [public/install.sh](./public/install.sh) is the official prebuilt installer and defaults to `x86_64-unknown-linux-musl`
- [scripts/install.sh](./scripts/install.sh) builds from local source and supports Linux and macOS
- If you build your own archive, you can still use `public/install.sh --asset-url <URL>`
- Manual packaging uses [scripts/package-release.sh](./scripts/package-release.sh)

Examples:

```bash
./scripts/package-release.sh --target x86_64-unknown-linux-musl
./scripts/package-release.sh --target aarch64-apple-darwin
```

CI runs on Linux and macOS.
The official release workflow publishes one Linux artifact: `mycodex-x86_64-unknown-linux-musl.tar.gz`.

## Development

```bash
cargo build --release
cargo test
```

Desktop shell:

```bash
cd apps/desktop
npm install
npm run tauri:dev
```

The desktop shell talks to the daemon over:

- `POST /api/app/pairings/request`
- `GET /api/app/pairings/{pairing_id}`
- authenticated WebSocket `/ws?token=...`

For manual Linux service setup, use [deploy/systemd/mycodex.service](./deploy/systemd/mycodex.service) as a starting point.
