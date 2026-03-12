# MyCodex

**语言:** [English](./README.md) | 简体中文

把 Codex 放在你自己控制的服务器上，然后从 Mac、Telegram 和手机继续同一个任务。

我做 MyCodex，是因为我想把 Codex 固定在自己的机器或 VPS 上跑，但自己可以在不同设备之间来回切换。一个 workspace 里可以放多个一级 Git 仓库，每个 repo 都保持隔离，也都可以有多个 thread。

macOS app 还在开发中，但方向很明确：人在电脑前时就在 Mac 上干活，离开工位之后，继续用 Telegram 或另一个已配对的客户端把同一个任务接着做下去。

<img src="./docs/media/telegram-flow.gif" alt="Telegram workflow demo" width="360" />

Telegram 这一侧现在已经是这样工作的：发起一个 thread，在聊天里审批敏感命令，然后在同一个地方收到执行结果。

## 现在它能做什么

- 一个 workspace 里可以放多个 repo，但不会把它们的 Codex 状态混在一起
- 每个 repo 可以开多个 thread
- Telegram thread 和 app thread 分开管理
- 命令审批和补丁审批会回到发起它们的入口
- 先在 Mac 上干活，之后再拿手机接着做，会顺很多

## 快速开始

先准备这些：

- 可用的 `codex` CLI
- Codex 登录态或 `OPENAI_API_KEY`
- `git`
- 如果要启用 Telegram，再准备一个 Telegram Bot Token

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

`onboard` 会做这些事：

- 可选校验 Telegram bot token
- 让你选择 workspace 路径
- 可选写入 `OPENAI_API_KEY`
- 可选开启远程 app gateway
- 可选启用已经安装的服务

## macOS App 形态

- 仅客户端模式：只启动 app 客户端，连接远程的 MyCodex server
- 仅服务器模式：只在 Mac 上启动 MyCodex server，供其他设备配对接入
- 同时启动模式：同一台 Mac 同时启动 server 和 client，本机写代码，离开工位后继续用手机接管

几条 macOS 相关说明：

- 本地 Host 会把生成的 `config.toml`、state、workspace 和日志都放进 app 自己的数据目录，不走系统安装版默认路径
- 本地 Host 可以完全关闭 Telegram，只保留 app gateway 和 Codex 主链路
- 本地 Host 的网络暴露是显式开关：`Local only` 绑定 `127.0.0.1`，`Allow LAN devices` 绑定 `0.0.0.0`，并展示给手机配对用的局域网地址

## 我自己会怎么用

1. 在 Mac 上以“同时启动模式”运行 MyCodex。
2. 在桌面 app 里，针对 workspace 里的某个 repo 本地工作。
3. 离开 Mac 之后，通过 Telegram 或另一个已配对的 app 在手机上继续同一个任务。
4. repo 隔离、thread 状态和审批链路都会继续绑定在发起它们的入口面上。

## 仓库结构

- `apps/server`：Rust daemon、Telegram 适配层、app gateway 和 CLI
- `apps/desktop`：Tauri + React 客户端骨架，覆盖桌面端、Android 和 iOS
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

## 基本模型

- `workspace`：装一级仓库的目录
- `repo`：运行时隔离边界
- `thread`：某个 repo 内的一次 Codex 会话
- `surface`：你正在使用的入口，当前有 `telegram` 和 `app`

切换 repo 不会继承另一个 repo 的运行时上下文。
Telegram thread 和 app thread 不会出现在对方的 thread 列表里。

Telegram 默认访问模式是 `pairing`。
第一次通过 Telegram 使用的流程：

1. 安装 MyCodex
2. 运行 `/usr/local/bin/mycodex onboard`
3. 给 bot 发消息
4. 收到 pairing code
5. 在服务器上执行 `/usr/local/bin/mycodex pairing approve <CODE>`

App 使用流程：

1. 在 `onboard` 时开启 app gateway，或者手动设置 `app.enabled = true`
2. 启动 daemon
3. 在桌面端或移动端打开 app 并申请 pairing code
4. 在服务器上执行 `/usr/local/bin/mycodex app pairing approve <CODE>`
5. 用签发下来的 bearer token 连接 app

纯 server 形态的 token 管理：

- pairing 仍然是默认推荐流程。
- 如果你是在 Linux 或 macOS 上以纯 daemon 形态运行 MyCodex，也可以直接在 server 上手动签发或轮换 APP token：

```bash
/usr/local/bin/mycodex app devices create --label "MacBook Pro"
/usr/local/bin/mycodex app devices rotate <DEVICE_ID>
/usr/local/bin/mycodex app devices revoke <DEVICE_ID>
/usr/local/bin/mycodex app devices list
```

- `create` 和 `rotate` 只会在当次输出一次新的 bearer token。server 只保存它的 hash，所以旧 token 之后无法再次展示。
- 如果你要让当前 token 失效，用 `rotate` 或 `revoke`。

桌面 app 里的几个主要页面：

- `Workbench`：主工作区，负责 workspace、repo、thread 和 composer 流程
- `Settings`：承载 server URL、token、pairing、设备标签和诊断信息
- `Host`：桌面专属页面，承载本地 server 生命周期、LAN 模式、配置路径和日志

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

## 打包与发布

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

客户端：

```bash
cd apps/desktop
npm install
npm run tauri:dev
```

移动端：

```bash
cd apps/desktop
npm run tauri:android:dev
npm run tauri:android:build
npm run tauri:ios:dev
npm run tauri:ios:build
```

说明：

- Android 和 iOS 的宿主工程已经落在 `apps/desktop/src-tauri/gen/android` 和 `apps/desktop/src-tauri/gen/apple`
- Android 构建默认允许连到局域网里的 HTTP / WebSocket daemon，便于本机和真机联调
- iOS 不提交签名材料；每个开发者在本地用自己的 Apple 账号签名和打包

桌面端会通过下面这些接口连接 daemon：

- `POST /api/app/pairings/request`
- `GET /api/app/pairings/{pairing_id}`
- 带认证信息的 WebSocket `/ws?token=...`

如果你要手动部署 Linux 服务，可以把 [deploy/systemd/mycodex.service](./deploy/systemd/mycodex.service) 当成起点。
