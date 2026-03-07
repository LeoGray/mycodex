#!/usr/bin/env bash
set -euo pipefail

DEFAULT_GITHUB_REPO="${MYCODEX_DEFAULT_GITHUB_REPO:-}"
RELEASE_VERSION="${MYCODEX_RELEASE_VERSION:-latest}"
RELEASE_ASSET_URL="${MYCODEX_RELEASE_ASSET_URL:-}"
RELEASE_TARGET_TRIPLE="${MYCODEX_RELEASE_TARGET_TRIPLE:-}"
RELEASE_BINARY_NAME="mycodex"

INSTALL_BIN="/usr/local/bin/mycodex"
CONFIG_DIR="/etc/mycodex"
CONFIG_PATH="${CONFIG_DIR}/config.toml"
ENV_PATH="${CONFIG_DIR}/mycodex.env"
SERVICE_PATH="/etc/systemd/system/mycodex.service"
WORKSPACE_ROOT="/srv/workspace"
STATE_DIR="/var/lib/mycodex"
RUN_USER="${SUDO_USER:-$(id -un)}"
RUN_GROUP=""
TELEGRAM_BOT_TOKEN="${MYCODEX_TELEGRAM_BOT_TOKEN:-}"
TELEGRAM_USER_ID="${MYCODEX_TELEGRAM_USER_ID:-}"
TELEGRAM_CHAT_ID="${MYCODEX_TELEGRAM_CHAT_ID:-}"
OPENAI_API_KEY_VALUE="${OPENAI_API_KEY:-}"
CODEX_BIN="${MYCODEX_CODEX_BIN:-}"
CODEX_MODEL="${MYCODEX_CODEX_MODEL:-}"
POLL_TIMEOUT_SECONDS=30
STREAM_EDIT_INTERVAL_MS=1200
MAX_INLINE_DIFF_CHARS=6000
CLONE_TIMEOUT_SEC=600
ALLOW_SSH=true
ALLOW_HTTPS=true
START_SERVICE=1

TEMP_DIR=""

usage() {
  cat <<'EOF'
Usage:
  curl -fsSL https://your-domain.example/install.sh | bash -s -- [options]

This installer is for users who do not need to clone the repository.
It downloads a prebuilt MyCodex release and installs it on Linux.

Required:
  --telegram-bot-token TOKEN
  --telegram-user-id ID

Optional:
  --github-repo OWNER/REPO
  --release-version TAG_OR_latest
  --asset-url URL
  --target-triple TARGET
  --telegram-chat-id ID
  --openai-api-key KEY
  --run-user USER
  --run-group GROUP
  --workspace-root PATH
  --state-dir PATH
  --install-bin PATH
  --config-path PATH
  --env-path PATH
  --service-path PATH
  --codex-bin PATH_OR_CMD
  --codex-model MODEL
  --poll-timeout-seconds N
  --stream-edit-interval-ms N
  --max-inline-diff-chars N
  --clone-timeout-sec N
  --disable-ssh
  --disable-https
  --no-start
  -h, --help
EOF
}

log() {
  printf '[mycodex-public-install] %s\n' "$*"
}

die() {
  printf '[mycodex-public-install] ERROR: %s\n' "$*" >&2
  exit 1
}

cleanup() {
  if [[ -n "${TEMP_DIR}" && -d "${TEMP_DIR}" ]]; then
    rm -rf "${TEMP_DIR}"
  fi
}

trap cleanup EXIT

run_privileged() {
  if [[ "${EUID}" -eq 0 ]]; then
    "$@"
  else
    sudo "$@"
  fi
}

run_as_user() {
  local user="$1"
  shift
  if [[ "$(id -un)" == "${user}" ]]; then
    "$@"
  elif [[ "${EUID}" -eq 0 ]]; then
    sudo -u "${user}" -- "$@"
  else
    sudo -u "${user}" -- "$@"
  fi
}

toml_escape() {
  printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'
}

detect_target_triple() {
  if [[ -n "${RELEASE_TARGET_TRIPLE}" ]]; then
    printf '%s\n' "${RELEASE_TARGET_TRIPLE}"
    return
  fi

  local arch
  arch="$(uname -m)"
  case "${arch}" in
    x86_64|amd64) arch="x86_64" ;;
    aarch64|arm64) arch="aarch64" ;;
    *)
      die "unsupported Linux architecture: ${arch}"
      ;;
  esac

  local libc_suffix="unknown-linux-musl"
  if command -v ldd >/dev/null 2>&1; then
    if ldd --version 2>&1 | grep -qi 'musl'; then
      libc_suffix="unknown-linux-musl"
    else
      libc_suffix="unknown-linux-gnu"
    fi
  fi

  printf '%s-%s\n' "${arch}" "${libc_suffix}"
}

resolve_release_asset_url() {
  if [[ -n "${RELEASE_ASSET_URL}" ]]; then
    printf '%s\n' "${RELEASE_ASSET_URL}"
    return
  fi

  local repo="${DEFAULT_GITHUB_REPO}"
  if [[ -n "${MYCODEX_RELEASE_GITHUB_REPO:-}" ]]; then
    repo="${MYCODEX_RELEASE_GITHUB_REPO}"
  fi
  if [[ -n "${GITHUB_REPO_OVERRIDE:-}" ]]; then
    repo="${GITHUB_REPO_OVERRIDE}"
  fi
  [[ -n "${repo}" ]] || die "missing GitHub repo; pass --github-repo or set MYCODEX_DEFAULT_GITHUB_REPO when hosting this script"

  local target
  target="$(detect_target_triple)"
  local asset_name="${RELEASE_BINARY_NAME}-${target}.tar.gz"

  if [[ "${RELEASE_VERSION}" == "latest" ]]; then
    printf 'https://github.com/%s/releases/latest/download/%s\n' "${repo}" "${asset_name}"
  else
    printf 'https://github.com/%s/releases/download/%s/%s\n' "${repo}" "${RELEASE_VERSION}" "${asset_name}"
  fi
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --github-repo)
      GITHUB_REPO_OVERRIDE="$2"
      shift 2
      ;;
    --release-version)
      RELEASE_VERSION="$2"
      shift 2
      ;;
    --asset-url)
      RELEASE_ASSET_URL="$2"
      shift 2
      ;;
    --target-triple)
      RELEASE_TARGET_TRIPLE="$2"
      shift 2
      ;;
    --telegram-bot-token)
      TELEGRAM_BOT_TOKEN="$2"
      shift 2
      ;;
    --telegram-user-id)
      TELEGRAM_USER_ID="$2"
      shift 2
      ;;
    --telegram-chat-id)
      TELEGRAM_CHAT_ID="$2"
      shift 2
      ;;
    --openai-api-key)
      OPENAI_API_KEY_VALUE="$2"
      shift 2
      ;;
    --run-user)
      RUN_USER="$2"
      shift 2
      ;;
    --run-group)
      RUN_GROUP="$2"
      shift 2
      ;;
    --workspace-root)
      WORKSPACE_ROOT="$2"
      shift 2
      ;;
    --state-dir)
      STATE_DIR="$2"
      shift 2
      ;;
    --install-bin)
      INSTALL_BIN="$2"
      shift 2
      ;;
    --config-path)
      CONFIG_PATH="$2"
      CONFIG_DIR="$(dirname "${CONFIG_PATH}")"
      shift 2
      ;;
    --env-path)
      ENV_PATH="$2"
      CONFIG_DIR="$(dirname "${ENV_PATH}")"
      shift 2
      ;;
    --service-path)
      SERVICE_PATH="$2"
      shift 2
      ;;
    --codex-bin)
      CODEX_BIN="$2"
      shift 2
      ;;
    --codex-model)
      CODEX_MODEL="$2"
      shift 2
      ;;
    --poll-timeout-seconds)
      POLL_TIMEOUT_SECONDS="$2"
      shift 2
      ;;
    --stream-edit-interval-ms)
      STREAM_EDIT_INTERVAL_MS="$2"
      shift 2
      ;;
    --max-inline-diff-chars)
      MAX_INLINE_DIFF_CHARS="$2"
      shift 2
      ;;
    --clone-timeout-sec)
      CLONE_TIMEOUT_SEC="$2"
      shift 2
      ;;
    --disable-ssh)
      ALLOW_SSH=false
      shift
      ;;
    --disable-https)
      ALLOW_HTTPS=false
      shift
      ;;
    --no-start)
      START_SERVICE=0
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "unknown option: $1"
      ;;
  esac
done

[[ "$(uname -s)" == "Linux" ]] || die "this installer only supports Linux"
command -v curl >/dev/null 2>&1 || die "curl is required"
command -v tar >/dev/null 2>&1 || die "tar is required"
command -v systemctl >/dev/null 2>&1 || die "systemctl is required"
command -v install >/dev/null 2>&1 || die "install is required"
command -v mktemp >/dev/null 2>&1 || die "mktemp is required"
if [[ "${EUID}" -ne 0 ]] && ! command -v sudo >/dev/null 2>&1; then
  die "sudo is required when not running as root"
fi

[[ -n "${TELEGRAM_BOT_TOKEN}" ]] || die "--telegram-bot-token is required"
[[ -n "${TELEGRAM_USER_ID}" ]] || die "--telegram-user-id is required"
id "${RUN_USER}" >/dev/null 2>&1 || die "run user does not exist: ${RUN_USER}"
if [[ -z "${RUN_GROUP}" ]]; then
  RUN_GROUP="$(id -gn "${RUN_USER}")"
fi
getent group "${RUN_GROUP}" >/dev/null 2>&1 || die "run group does not exist: ${RUN_GROUP}"

TEMP_DIR="$(mktemp -d)"
ASSET_URL="$(resolve_release_asset_url)"
ARCHIVE_PATH="${TEMP_DIR}/mycodex-release.tar.gz"
EXTRACT_DIR="${TEMP_DIR}/extract"
mkdir -p "${EXTRACT_DIR}"

log "downloading release asset ${ASSET_URL}"
curl -fsSL --retry 3 --connect-timeout 10 "${ASSET_URL}" -o "${ARCHIVE_PATH}" || die "failed to download release asset"

log "extracting release archive"
tar -xzf "${ARCHIVE_PATH}" -C "${EXTRACT_DIR}" || die "failed to extract release archive"
BINARY_PATH="$(find "${EXTRACT_DIR}" -type f -name "${RELEASE_BINARY_NAME}" | head -n 1 || true)"
[[ -n "${BINARY_PATH}" ]] || die "release archive did not contain ${RELEASE_BINARY_NAME}"
chmod +x "${BINARY_PATH}"

if [[ -z "${CODEX_BIN}" ]]; then
  printf -v resolve_codex_cmd 'command -v codex'
  CODEX_BIN="$(run_as_user "${RUN_USER}" bash -lc "${resolve_codex_cmd}" || true)"
fi
[[ -n "${CODEX_BIN}" ]] || die "failed to resolve codex binary; pass --codex-bin explicitly"
if [[ "${CODEX_BIN}" != /* ]]; then
  printf -v resolve_named_codex_cmd 'command -v %q' "${CODEX_BIN}"
  RESOLVED_CODEX_BIN="$(run_as_user "${RUN_USER}" bash -lc "${resolve_named_codex_cmd}" || true)"
  [[ -n "${RESOLVED_CODEX_BIN}" ]] || die "codex binary not found for user ${RUN_USER}: ${CODEX_BIN}"
  CODEX_BIN="${RESOLVED_CODEX_BIN}"
fi
[[ -x "${CODEX_BIN}" ]] || die "codex binary is not executable: ${CODEX_BIN}"

log "installing binary to ${INSTALL_BIN}"
run_privileged install -d -m 0755 "$(dirname "${INSTALL_BIN}")"
run_privileged install -m 0755 "${BINARY_PATH}" "${INSTALL_BIN}"

log "preparing directories"
run_privileged install -d -m 0755 "${CONFIG_DIR}"
run_privileged install -d -m 0755 -o "${RUN_USER}" -g "${RUN_GROUP}" "${STATE_DIR}"
run_privileged install -d -m 0755 -o "${RUN_USER}" -g "${RUN_GROUP}" "${WORKSPACE_ROOT}"

CONFIG_TMP="$(mktemp)"
ENV_TMP="$(mktemp)"
SERVICE_TMP="$(mktemp)"
trap 'rm -f "${CONFIG_TMP}" "${ENV_TMP}" "${SERVICE_TMP}"; cleanup' EXIT

{
  echo "[workspace]"
  printf 'root = "%s"\n' "$(toml_escape "${WORKSPACE_ROOT}")"
  echo
  echo "[telegram]"
  printf 'bot_token = "%s"\n' "$(toml_escape "${TELEGRAM_BOT_TOKEN}")"
  printf 'allowed_user_id = %s\n' "${TELEGRAM_USER_ID}"
  if [[ -n "${TELEGRAM_CHAT_ID}" ]]; then
    printf 'allowed_chat_id = %s\n' "${TELEGRAM_CHAT_ID}"
  fi
  printf 'poll_timeout_seconds = %s\n' "${POLL_TIMEOUT_SECONDS}"
  echo
  echo "[codex]"
  printf 'bin = "%s"\n' "$(toml_escape "${CODEX_BIN}")"
  if [[ -n "${CODEX_MODEL}" ]]; then
    printf 'model = "%s"\n' "$(toml_escape "${CODEX_MODEL}")"
  fi
  echo
  echo "[state]"
  printf 'dir = "%s"\n' "$(toml_escape "${STATE_DIR}")"
  echo
  echo "[ui]"
  printf 'stream_edit_interval_ms = %s\n' "${STREAM_EDIT_INTERVAL_MS}"
  printf 'max_inline_diff_chars = %s\n' "${MAX_INLINE_DIFF_CHARS}"
  echo
  echo "[git]"
  printf 'clone_timeout_sec = %s\n' "${CLONE_TIMEOUT_SEC}"
  printf 'allow_ssh = %s\n' "${ALLOW_SSH}"
  printf 'allow_https = %s\n' "${ALLOW_HTTPS}"
} > "${CONFIG_TMP}"

{
  echo "# Optional Codex auth environment for MyCodex"
  if [[ -n "${OPENAI_API_KEY_VALUE}" ]]; then
    printf 'OPENAI_API_KEY=%s\n' "${OPENAI_API_KEY_VALUE}"
  else
    echo "# OPENAI_API_KEY=replace-me"
  fi
} > "${ENV_TMP}"

{
  echo "[Unit]"
  echo "Description=MyCodex Telegram multi-repo gateway"
  echo "After=network-online.target"
  echo "Wants=network-online.target"
  echo
  echo "[Service]"
  echo "Type=simple"
  printf 'User=%s\n' "${RUN_USER}"
  printf 'Group=%s\n' "${RUN_GROUP}"
  printf 'WorkingDirectory=%s\n' "${STATE_DIR}"
  echo "Environment=RUST_LOG=info"
  printf 'EnvironmentFile=-%s\n' "${ENV_PATH}"
  printf 'ExecStart=%s serve --config %s\n' "${INSTALL_BIN}" "${CONFIG_PATH}"
  echo "Restart=always"
  echo "RestartSec=3"
  echo "NoNewPrivileges=true"
  echo "PrivateTmp=true"
  echo
  echo "[Install]"
  echo "WantedBy=multi-user.target"
} > "${SERVICE_TMP}"

log "writing configuration to ${CONFIG_PATH}"
run_privileged install -m 0640 -o root -g "${RUN_GROUP}" "${CONFIG_TMP}" "${CONFIG_PATH}"
log "writing environment file to ${ENV_PATH}"
run_privileged install -m 0640 -o root -g "${RUN_GROUP}" "${ENV_TMP}" "${ENV_PATH}"
log "writing systemd service to ${SERVICE_PATH}"
run_privileged install -m 0644 "${SERVICE_TMP}" "${SERVICE_PATH}"

log "reloading systemd"
run_privileged systemctl daemon-reload

SERVICE_NAME="$(basename "${SERVICE_PATH}")"
if [[ "${START_SERVICE}" -eq 1 ]]; then
  log "running mycodex check before starting service"
  printf -v check_cmd 'set -a && source %q && set +a && %q check --config %q' \
    "${ENV_PATH}" "${INSTALL_BIN}" "${CONFIG_PATH}"
  run_as_user "${RUN_USER}" bash -lc "${check_cmd}"

  log "enabling and starting ${SERVICE_NAME}"
  run_privileged systemctl enable --now "${SERVICE_NAME}"
else
  log "installation finished without starting the service (--no-start)"
fi

cat <<EOF

MyCodex release installation complete.

Summary:
  run user:         ${RUN_USER}
  run group:        ${RUN_GROUP}
  binary:           ${INSTALL_BIN}
  config:           ${CONFIG_PATH}
  env file:         ${ENV_PATH}
  service:          ${SERVICE_PATH}
  workspace root:   ${WORKSPACE_ROOT}
  state dir:        ${STATE_DIR}
  codex bin:        ${CODEX_BIN}
  asset url:        ${ASSET_URL}

Useful commands:
  sudo systemctl status ${SERVICE_NAME}
  sudo journalctl -u ${SERVICE_NAME} -f
  ${INSTALL_BIN} check --config ${CONFIG_PATH}
EOF
