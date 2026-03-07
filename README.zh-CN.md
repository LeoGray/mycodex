# MyCodex

**语言:** [English](./README.md) | 简体中文

`MyCodex` 是一个跑在 Linux 服务器上的 Telegram 远程编码网关。

它把工作区里的多个 Git 仓库组织起来，并通过本机安装的 `codex` CLI 为每个仓库提供独立的 Codex 运行时和多线程会话管理。

## 设计目标

- 运行在 Linux 服务器上。
- 通过 Telegram 私聊远程驱动 Codex。
- 一个 `workspace` 下支持多个一级仓库。
- 每个 `repo` 独立管理自己的 Codex runtime。
- 每个 `repo` 可以有多个 Codex thread。
- 用户可以在 Telegram 里做仓库切换、线程切换、clone、新开线程、审批命令和补丁。

## 核心模型

系统内部是三层模型：

- `workspace`
  - 仓库容器目录，例如 `/srv/workspace`
  - 启动时只扫描它下面的一级子目录
- `repo`
  - 真正的工作单元
  - 每个 repo 绑定一个独立的 Codex runtime 边界
  - 切换 repo 时，不复用另一个 repo 的 runtime
- `thread`
  - repo 内部的 Codex 会话
  - 一个 repo 可以有多个 thread
  - 一个 repo 同时只有一个 active thread

这意味着：

- repo A 和 repo B 不会共享同一个 Codex 进程上下文
- repo A 下面可以保留多个历史 thread
- 切回 repo A 时，会继续它自己的 active thread，而不是 repo B 的上下文

## 当前能力

当前版本已经实现：

- Telegram long polling
- 单用户 allowlist
- workspace 一级仓库扫描
- 在 workspace 下 clone 新仓库
- `/repo use` 切换当前 repo
- 每个 repo 下 `/thread new` 和 `/thread use`
- 文本消息发送到当前 repo 的当前 thread
- Codex 命令审批
- Codex 文件修改审批
- 本地状态持久化

## 当前限制

这是一个 MVP，目前有这些边界：

- 只支持 Telegram
- 只支持单用户
- 只扫描 workspace 的一级子目录
- 不处理嵌套 repo、submodule、git worktree
- `clone` 只走默认分支
- 不负责管理 Codex 登录流程
- 不提供 Web UI
- 不提供多用户隔离

## 运行前准备

部署前需要准备这些条件：

1. 一台 Linux 服务器
2. 已安装可用的 `codex` CLI
3. 可用的 Codex 认证环境
4. 一个 Telegram Bot Token
5. 服务器上已经能正常使用 `git`
6. 一个用于放多个仓库的 workspace 目录

推荐你在服务器上先手动确认以下命令都能工作：

```bash
codex --version
codex app-server --help
git --version
```

## Codex 认证

`MyCodex` 不管理 Codex 登录，只消费服务器上已经可用的认证环境。

推荐方式是通过环境变量提供：

```bash
export OPENAI_API_KEY=...
```

如果你的服务器上 `codex` 已经有现成登录态，也可以直接复用。`MyCodex` 的 `check` 命令会验证：

- Telegram token 是否可用
- `codex app-server` 是否能正常初始化
- workspace 是否存在
- workspace 下能否扫描仓库

## 配置文件

从样例配置开始：

[config/config.example.toml](./config/config.example.toml)

示例：

```toml
[workspace]
root = "/srv/workspace"

[telegram]
bot_token = "123456:replace-me"
access_mode = "pairing"
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

### 关键配置项说明

- `workspace.root`
  - 仓库容器目录
  - 例如 `/srv/workspace`
- `telegram.bot_token`
  - Telegram Bot Token
- `telegram.access_mode`
  - 默认推荐值是 `pairing`
  - 在 pairing 模式下，未知用户先拿 pairing code，而不是直接控制 bot
- `codex.bin`
  - `codex` 可执行文件路径或命令名
- `codex.model`
  - 可选
  - 如果设置，则 turn 会显式使用这个模型
- `state.dir`
  - MyCodex 的状态目录
  - 会保存 repo/thread 映射、当前 active repo/thread、临时 patch 文件等
- `ui.stream_edit_interval_ms`
  - Telegram 流式编辑节流时间
- `ui.max_inline_diff_chars`
  - 超过这个长度的 diff 会发送成 `.patch` 文件，而不是直接内联
- `git.clone_timeout_sec`
  - `git clone` 超时时间
- `git.allow_ssh`
  - 是否允许 SSH URL
- `git.allow_https`
  - 是否允许 HTTPS URL

## 本地运行

开发环境下可以这样运行：

```bash
cargo build --release
./target/release/mycodex check --config ./config/config.example.toml
./target/release/mycodex serve --config ./config/config.example.toml
```

`check` 会做启动前检查。`serve` 会正式进入 Telegram 轮询和 Codex 事件循环。

## 安装入口

现在仓库里有两个不同职责的安装脚本：

- [scripts/install.sh](./scripts/install.sh)
  - 给“已经 clone 仓库的人”使用
  - 从当前源码树本地构建并安装
- [public/install.sh](./public/install.sh)
  - 给“官网一键安装”使用
  - 只下载预构建 release 二进制，不依赖本地源码树

这两个入口现在是明确分开的，不再混在一个脚本里。

## 场景 1：从源码安装

这适合：

- 你已经 `git clone` 了仓库
- 你想直接从当前源码树构建
- 你在开发、测试或者手动部署

示例：

```bash
./scripts/install.sh \
  --install-systemd
```

这个模式会：

- 在当前源码树里执行 `cargo build --release`
- 安装 release 二进制
- 生成配置模板和环境变量模板
- 可选安装 systemd 服务文件
- 打印下一步 onboarding 命令

如果脚本检测到机器上已经有安装，它会自动切换到 update 模式：

- 保留现有配置和 env 文件
- 沿用当前 systemd 方案
- 如果服务已经在运行，会自动重启

## 场景 2：从预构建 release 安装

这适合：

- 用户不想 clone 仓库
- 用户只想执行一条安装命令
- 你已经把 release 二进制发到了 GitHub Releases 或自己的下载地址

推荐的一行安装命令：

```bash
curl -fsSL https://raw.githubusercontent.com/LeoGray/mycodex/main/public/install.sh | bash
```

这个命令只负责安装，不负责 Telegram 产品配置。配置在下一步的 `mycodex onboard` 里完成。

高级非交互示例：

```bash
curl -fsSL https://raw.githubusercontent.com/LeoGray/mycodex/main/public/install.sh | bash -s -- \
  --install-systemd
```

也可以直接指定完整下载地址：

```bash
curl -fsSL https://raw.githubusercontent.com/LeoGray/mycodex/main/public/install.sh | bash -s -- \
  --asset-url https://example.com/mycodex-x86_64-unknown-linux-gnu.tar.gz
```

这个模式会：

- 根据当前 Linux 架构推断目标 release 产物
- 下载对应的压缩包
- 解压出 `mycodex` 二进制
- 后续仍然是纯安装流程，不做 Telegram 业务配置

如果脚本检测到机器上已经有安装，它会自动切换到 update 模式：

- 保留现有安装布局
- 不会重复询问是否安装 systemd
- 如果服务已经在运行，会自动重启

### OpenClaw 风格的一条命令体验

对于当前这个开源项目，可以直接用 GitHub Raw：

```bash
curl -fsSL https://raw.githubusercontent.com/LeoGray/mycodex/main/public/install.sh | bash
```

公开安装器现在已经默认指向 `LeoGray/mycodex`。如果以后你 fork 或改名，只需要改一下 `public/install.sh` 顶部的默认 GitHub repo。

### 默认行为

两个安装脚本都会默认：

- 把服务跑在当前用户上，而不是强制创建一个系统用户
- 尽量复用当前用户已有的：
  - `codex` 认证环境
  - git 凭证
  - SSH key
- 使用这些默认路径：
  - 二进制：`/usr/local/bin/mycodex`
  - 配置：`/etc/mycodex/config.toml`
  - 环境变量：`/etc/mycodex/mycodex.env`
  - 服务：`/etc/systemd/system/mycodex.service`
  - workspace：`/srv/workspace`
  - state：`/var/lib/mycodex`

### `scripts/install.sh` 常用参数

源码安装脚本支持：

- `--update`
- `--run-user`
- `--run-group`
- `--workspace-root`
- `--state-dir`
- `--install-bin`
- `--config-path`
- `--env-path`
- `--service-path`
- `--install-systemd`
- `--skip-systemd`
- `--skip-build`

### `public/install.sh` 常用参数

官网安装脚本支持：

- `--update`
- `--github-repo`
- `--release-version`
- `--asset-url`
- `--target-triple`
- `--run-user`
- `--run-group`
- `--workspace-root`
- `--state-dir`
- `--install-bin`
- `--config-path`
- `--env-path`
- `--service-path`
- `--install-systemd`
- `--skip-systemd`

### 环境变量回退

安装脚本支持这些环境变量：

- `MYCODEX_TELEGRAM_BOT_TOKEN`
- `MYCODEX_RELEASE_GITHUB_REPO`
- `MYCODEX_RELEASE_VERSION`
- `MYCODEX_RELEASE_ASSET_URL`
- `MYCODEX_RELEASE_TARGET_TRIPLE`
- `OPENAI_API_KEY`

## Release 打包

为了配合“预构建二进制安装”，仓库里还加了一个打包脚本：

[scripts/package-release.sh](./scripts/package-release.sh)

示例：

```bash
./scripts/package-release.sh --target x86_64-unknown-linux-gnu
./scripts/package-release.sh --target aarch64-unknown-linux-musl
```

它会输出：

- `dist/mycodex-x86_64-unknown-linux-gnu.tar.gz`
- `dist/mycodex-aarch64-unknown-linux-musl.tar.gz`

安装器的 release 模式默认就是按这个命名规则去找资产。

## GitHub Actions 构建与发布

仓库里现在已经带了两个 GitHub Actions 工作流：

- [ci.yml](./.github/workflows/ci.yml)
  - 在 GitHub-hosted `ubuntu-latest` runner 上执行 `cargo check` 和 `cargo test`
- [release.yml](./.github/workflows/release.yml)
  - 在打 tag 时自动构建 Linux release 资产
  - 当前会产出：
    - `mycodex-x86_64-unknown-linux-gnu.tar.gz`
    - `mycodex-x86_64-unknown-linux-musl.tar.gz`
  - 并自动上传到 GitHub Releases

这意味着后面你可以把安装入口做成：

```bash
curl -fsSL https://raw.githubusercontent.com/LeoGray/mycodex/main/public/install.sh | bash
mycodex onboard
```

目前 release workflow 先覆盖最常见的 x86_64 Linux 服务器。ARM64 Linux 资产后面可以再补。

## Onboarding

安装完成后，下一步应该运行：

```bash
mycodex onboard
```

当前 onboarding 会交互式完成这些事情：

- 用 `getMe` 验证 Telegram bot token
- 选择 workspace 路径
  - 默认：`$HOME/workspace`
  - 可以手动指定已有路径
  - 如果目录不存在，可以直接创建
- 可选填写 `OPENAI_API_KEY`
- 如果 systemd 服务已经安装，可选立即启动

现在推荐的产品流是：

```bash
curl -fsSL https://raw.githubusercontent.com/LeoGray/mycodex/main/public/install.sh | bash
mycodex onboard
```

## Pairing

MyCodex 现在默认使用 `telegram.access_mode = "pairing"`。

这意味着：

- 未配对的 Telegram 用户不能直接控制 bot
- 他们只会收到 pairing code
- 服务器管理员需要显式批准

服务器侧命令：

```bash
mycodex pairing list
mycodex pairing approve <CODE>
mycodex pairing reject <CODE>
```

第一次使用的流程应该是：

1. 安装 MyCodex
2. 运行 `mycodex onboard`
3. 用 Telegram 给 bot 发消息
4. 收到 pairing code
5. 在服务器上运行 `mycodex pairing approve <CODE>`

## systemd 部署

可以从这个模板开始：

[deploy/systemd/mycodex.service](./deploy/systemd/mycodex.service)

建议目录布局：

- `/usr/local/bin/mycodex`
- `/etc/mycodex/config.toml`
- `/etc/mycodex/mycodex.env`
- `/var/lib/mycodex`
- `/srv/workspace`

### 1. 拷贝二进制和配置

```bash
sudo mkdir -p /etc/mycodex
sudo mkdir -p /var/lib/mycodex
sudo mkdir -p /srv/workspace
sudo cp ./target/release/mycodex /usr/local/bin/mycodex
sudo cp ./config/config.example.toml /etc/mycodex/config.toml
```

### 2. 写环境变量文件

例如：

```bash
sudo tee /etc/mycodex/mycodex.env >/dev/null <<'EOF'
OPENAI_API_KEY=replace-me
EOF
```

### 3. 安装 systemd 服务

```bash
sudo cp ./deploy/systemd/mycodex.service /etc/systemd/system/mycodex.service
sudo systemctl daemon-reload
sudo systemctl enable --now mycodex
```

### 4. 查看日志

```bash
sudo journalctl -u mycodex -f
```

## Telegram 使用方式

### 基础命令

- `/start`
  - 显示帮助
- `/status`
  - 显示当前 active repo、active thread、runtime 状态、active turn、pending approval
- `/abort`
  - 中断当前 turn

### Repo 命令

- `/repo list`
  - 列出当前已注册的 repo
- `/repo use <name>`
  - 切换 active repo
- `/repo clone <git_url> [dir_name]`
  - clone 新 repo 到 workspace 下
- `/repo status`
  - 查看当前 repo 状态
- `/repo rescan`
  - 重新扫描 workspace 下已有 repo

### Thread 命令

- `/thread list`
  - 列出当前 repo 下的 thread
- `/thread new`
  - 在当前 repo 下新建一个 thread
- `/thread use <thread>`
  - 切换到当前 repo 下某个旧 thread
  - `<thread>` 可以是列表序号，也可以是本地 thread ID 前缀
- `/thread status`
  - 查看当前 repo 的 active thread 状态

### 普通文本消息

普通文本会发送到：

- 当前 active repo
- 当前 active thread

如果当前 repo 还没有 thread，第一次普通文本会自动新建 thread。

## 一个典型使用流程

### 场景 1：第一次进入 repo A

```text
/repo rescan
/repo use repo-a
修一下这个仓库的 CI 错误
```

此时：

- MyCodex 会启动 repo A 对应的 Codex runtime
- 如果 repo A 还没有 thread，会自动新建一个
- 后续文本都进入 repo A 的这个 active thread

### 场景 2：repo A 上下文太长，开一个新 thread

```text
/thread new
重新从头整理一下这个功能的实现方案
```

此时：

- 仍然在 repo A
- 不会新起另一个 repo runtime
- 只是在 repo A 下面创建一个新的 Codex thread

### 场景 3：切到 repo B

```text
/repo use repo-b
帮我看一下这个仓库的部署脚本
```

此时：

- repo A runtime 会被停掉
- repo B runtime 会被启动
- repo B 使用它自己的 thread 集合
- 不会混用 repo A 的上下文

## 审批行为

当 Codex 要执行高风险命令或应用文件修改时，机器人会在 Telegram 里发审批消息。

### 命令审批

会展示：

- repo 名称
- thread 标题
- cwd
- command
- 原因

按钮：

- `Approve once`
- `Decline`
- `Abort turn`

### 文件审批

会展示：

- repo 名称
- thread 标题
- 变更路径
- diff 预览

如果 diff 太长，会额外发一个 `.patch` 文件。

按钮：

- `Approve patch`
- `Decline patch`

## 状态持久化

MyCodex 会在 `state.dir` 下面保存：

- repo catalog
- 每个 repo 的 thread catalog
- 当前 active repo
- 当前 active thread
- 当前 pending approval
- 当前进度消息 ID

服务重启后：

- 会保留 repo 和 thread 映射
- 会尝试恢复 active repo
- 不会恢复陈旧的 active turn
- 不会恢复陈旧的 pending approval

## 仓库扫描规则

启动时和 `/repo rescan` 时，只会扫描 `workspace.root` 的一级子目录。

例如：

```text
/srv/workspace
├── repo-a
├── repo-b
└── repo-c
```

只有 `repo-a`、`repo-b`、`repo-c` 这样的一级目录会被识别。

## clone 规则

当前 clone 行为固定为：

- 支持 `https://...`
- 支持 `ssh://...`
- 支持 `git@host:org/repo.git` 这种 scp 风格 SSH 地址
- 默认 clone 默认分支
- 默认目标目录取 URL basename
- 如果目录已存在，则直接拒绝
- Git 认证依赖服务器现有 SSH key 或 credential helper

## 故障排查

### `mycodex check` 失败

优先检查：

- Telegram token 是否正确
- `codex` 是否在 `PATH` 里
- `codex app-server` 是否能启动
- `OPENAI_API_KEY` 或现有登录态是否可用
- workspace 和 state 目录是否存在且可写

### Telegram 没有响应

优先检查：

- Bot Token 是否正确
- 是否已经成功完成 onboarding
- `mycodex pairing list` 里是否有待批准的 pairing request
- 服务日志里有没有 `telegram polling failed`

### clone 失败

优先检查：

- 服务器是否能访问 git 远端
- SSH key / 凭证是否可用
- 目标目录是否已存在
- URL 是否被 `git.allow_ssh` / `git.allow_https` 限制

### Codex 运行异常

优先检查：

- `codex --version`
- `codex app-server --help`
- `OPENAI_API_KEY`
- `journalctl -u mycodex -f`
