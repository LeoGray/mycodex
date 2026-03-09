# MyCodex

**语言:** [English](./README.md) | 简体中文

MyCodex 是一个面向 `codex` 的远程编码网关。
它把多个 Git 仓库放进同一个 workspace 里管理，同时保证 repo 之间的 Codex 运行时隔离，支持每个 repo 下的多线程会话，并提供 Telegram 和可选桌面 APP 两种控制入口。

核心点：

- Telegram 仍然是一级控制入口
- 开启后，桌面 APP 可以通过 HTTP 和 WebSocket 配对接入
- 一个 workspace 可以放多个一级仓库
- 每个 repo 都有独立的 Codex runtime 边界
- 每个 repo 可以有多个 thread
- Telegram thread 和 APP thread 相互隔离
- 命令审批和补丁审批会回到发起该 run 的入口面

## 快速开始

先准备好：

- 可用的 `codex` CLI
- Codex 登录态或 `OPENAI_API_KEY`
- `git`
- Telegram Bot Token

官方一键安装，适用于 x86_64 Linux：

```bash
curl -fsSL https://raw.githubusercontent.com/LeoGray/mycodex/main/public/install.sh | bash
/usr/local/bin/mycodex onboard
```

源码安装，适用于 Linux 和 macOS：

```bash
git clone https://github.com/LeoGray/mycodex.git
cd mycodex
./scripts/install.sh --install-service
/usr/local/bin/mycodex onboard
```

`onboard` 会完成：

- 校验 Telegram bot token
- 选择 workspace 路径
- 可选写入 `OPENAI_API_KEY`
- 可选开启远程 APP gateway
- 可选启用已安装的服务

## 仓库结构

- `apps/server`：Rust daemon、Telegram 适配层、APP gateway 和 CLI
- `apps/desktop`：Tauri + React 桌面客户端骨架
- `config`：示例配置
- `deploy`：服务定义
- `scripts`：安装和打包脚本

## 命令菜单

基础命令：

- `/start`
- `/status`
- `/abort`

Repo 命令：

- `/repo list`
- `/repo use <name>`
- `/repo clone <git_url> [dir_name]`
- `/repo status`
- `/repo rescan`

Thread 命令：

- `/thread list`
- `/thread new`
- `/thread use <thread>`
- `/thread status`

普通文本消息会始终发到当前 active repo 的当前 active thread。

## 工作模型

- `workspace`：装一级仓库的目录
- `repo`：运行时隔离边界
- `thread`：某个 repo 内的一次 Codex 会话
- `surface`：交互入口，当前有 `telegram` 和 `app`

切换 repo 不会继承另一个 repo 的运行时上下文。
Telegram thread 和 APP thread 不会出现在对方的 thread 列表里。

Telegram 默认访问模式是 `pairing`。
第一次通过 Telegram 使用的流程：

1. 安装 MyCodex
2. 运行 `/usr/local/bin/mycodex onboard`
3. 给 bot 发消息
4. 收到 pairing code
5. 在服务器上执行 `/usr/local/bin/mycodex pairing approve <CODE>`

桌面 APP 使用流程：

1. 在 `onboard` 时开启 APP gateway，或者手动设置 `app.enabled = true`
2. 启动 daemon
3. 打开桌面 APP 并申请 pairing code
4. 在服务器上执行 `/usr/local/bin/mycodex app pairing approve <CODE>`
5. 用签发下来的 bearer token 连接 APP

## 配置

从 [config/config.example.toml](./config/config.example.toml) 开始。

最重要的字段：

- `workspace.root`
- `telegram.bot_token`
- `telegram.access_mode`
- `app.enabled`
- `app.bind_addr`
- `app.public_base_url`
- `codex.bin`
- `state.dir`

默认路径：

- Linux
  - 配置：`/etc/mycodex/config.toml`
  - 环境变量：`/etc/mycodex/mycodex.env`
  - 服务：`/etc/systemd/system/mycodex.service`
  - 状态目录：`/var/lib/mycodex`
- macOS
  - 配置：`$HOME/.config/mycodex/config.toml`
  - 环境变量：`$HOME/.config/mycodex/mycodex.env`
  - 服务：`$HOME/Library/LaunchAgents/com.leogray.mycodex.plist`
  - 状态目录：`$HOME/.local/state/mycodex`

## 安装与发布说明

- [public/install.sh](./public/install.sh) 是官方预构建安装器，默认只发 `x86_64-unknown-linux-musl`
- [scripts/install.sh](./scripts/install.sh) 从本地源码构建，支持 Linux 和 macOS
- 如果你自己打好了归档包，也可以继续用 `public/install.sh --asset-url <URL>`
- 手动打包用 [scripts/package-release.sh](./scripts/package-release.sh)

示例：

```bash
./scripts/package-release.sh --target x86_64-unknown-linux-musl
./scripts/package-release.sh --target aarch64-apple-darwin
```

CI 会在 Linux 和 macOS 上跑。
官方 release workflow 只发布一个 Linux 产物：`mycodex-x86_64-unknown-linux-musl.tar.gz`。

## 开发

```bash
cargo build --release
cargo test
```

桌面端：

```bash
cd apps/desktop
npm install
npm run tauri:dev
```

桌面端会通过下面这些接口连接 daemon：

- `POST /api/app/pairings/request`
- `GET /api/app/pairings/{pairing_id}`
- 带认证信息的 WebSocket `/ws?token=...`

如果你要手动部署 Linux 服务，可以把 [deploy/systemd/mycodex.service](./deploy/systemd/mycodex.service) 当成起点。
