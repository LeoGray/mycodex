#!/usr/bin/env bash
set -euo pipefail

DEFAULT_GITHUB_REPO="${MYCODEX_DEFAULT_GITHUB_REPO:-LeoGray/mycodex}"
RELEASE_VERSION="${MYCODEX_RELEASE_VERSION:-latest}"
RELEASE_ASSET_URL="${MYCODEX_RELEASE_ASSET_URL:-}"
RELEASE_TARGET_TRIPLE="${MYCODEX_RELEASE_TARGET_TRIPLE:-}"
RELEASE_BINARY_NAME="mycodex"

INSTALL_BIN="/usr/local/bin/mycodex"
CONFIG_DIR="/etc/mycodex"
CONFIG_PATH="${CONFIG_DIR}/config.toml"
ENV_PATH="${CONFIG_DIR}/mycodex.env"
SERVICE_PATH="/etc/systemd/system/mycodex.service"
STATE_DIR="/var/lib/mycodex"
RUN_USER="${SUDO_USER:-$(id -un)}"
RUN_GROUP=""
RUN_HOME=""
WORKSPACE_ROOT=""
INSTALL_SYSTEMD=""
UPDATE_MODE=""
AUTO_DETECTED_UPDATE="false"

TEMP_DIR=""

usage() {
  cat <<'EOF'
Usage:
  curl -fsSL https://raw.githubusercontent.com/LeoGray/mycodex/main/public/install.sh | bash
  curl -fsSL https://raw.githubusercontent.com/LeoGray/mycodex/main/public/install.sh | bash -s -- [options]

This installer downloads a prebuilt MyCodex release and installs it on Linux.
Configuration happens later via:

  mycodex onboard

Optional:
  --update
  --github-repo OWNER/REPO
  --release-version TAG_OR_latest
  --asset-url URL
  --target-triple TARGET
  --run-user USER
  --run-group GROUP
  --workspace-root PATH
  --state-dir PATH
  --install-bin PATH
  --config-path PATH
  --env-path PATH
  --service-path PATH
  --install-systemd
  --skip-systemd
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

has_tty() {
  [[ -r /dev/tty && -w /dev/tty ]]
}

confirm() {
  local prompt="$1"
  local default_yes="${2:-true}"
  local suffix="[Y/n]"
  if [[ "${default_yes}" != "true" ]]; then
    suffix="[y/N]"
  fi

  if ! has_tty; then
    [[ "${default_yes}" == "true" ]]
    return
  fi

  local answer=""
  printf '%s %s ' "${prompt}" "${suffix}" > /dev/tty
  IFS= read -r answer < /dev/tty
  answer="$(printf '%s' "${answer}" | tr '[:upper:]' '[:lower:]')"
  if [[ -z "${answer}" ]]; then
    [[ "${default_yes}" == "true" ]]
    return
  fi
  [[ "${answer}" == "y" || "${answer}" == "yes" ]]
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
    *) die "unsupported Linux architecture: ${arch}" ;;
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
  if [[ -n "${GITHUB_REPO_OVERRIDE:-}" ]]; then
    repo="${GITHUB_REPO_OVERRIDE}"
  fi

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
    --update)
      UPDATE_MODE="true"
      shift
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
    --install-systemd)
      INSTALL_SYSTEMD="true"
      shift
      ;;
    --skip-systemd)
      INSTALL_SYSTEMD="false"
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
command -v install >/dev/null 2>&1 || die "install is required"
command -v mktemp >/dev/null 2>&1 || die "mktemp is required"
if [[ "${EUID}" -ne 0 ]] && ! command -v sudo >/dev/null 2>&1; then
  die "sudo is required when not running as root"
fi

id "${RUN_USER}" >/dev/null 2>&1 || die "run user does not exist: ${RUN_USER}"
if [[ -z "${RUN_GROUP}" ]]; then
  RUN_GROUP="$(id -gn "${RUN_USER}")"
fi
RUN_HOME="$(getent passwd "${RUN_USER}" | cut -d: -f6)"
[[ -n "${RUN_HOME}" ]] || die "failed to resolve home directory for ${RUN_USER}"
if [[ -z "${WORKSPACE_ROOT}" ]]; then
  WORKSPACE_ROOT="${RUN_HOME}/workspace"
fi

if [[ -z "${UPDATE_MODE}" ]]; then
  if [[ -x "${INSTALL_BIN}" || -f "${CONFIG_PATH}" || -f "${SERVICE_PATH}" ]]; then
    UPDATE_MODE="true"
    AUTO_DETECTED_UPDATE="true"
    log "detected existing installation; using update mode"
  else
    UPDATE_MODE="false"
  fi
fi

if [[ -z "${INSTALL_SYSTEMD}" ]]; then
  if [[ "${UPDATE_MODE}" == "true" ]]; then
    if [[ -f "${SERVICE_PATH}" ]]; then
      INSTALL_SYSTEMD="true"
    else
      INSTALL_SYSTEMD="false"
    fi
  else
    if confirm "Install a systemd service file?" true; then
      INSTALL_SYSTEMD="true"
    else
      INSTALL_SYSTEMD="false"
    fi
  fi
fi

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

if [[ ! -f "${CONFIG_PATH}" ]]; then
  {
    echo "[workspace]"
    printf 'root = "%s"\n' "$(toml_escape "${WORKSPACE_ROOT}")"
    echo
    echo "[telegram]"
    printf 'bot_token = ""\n'
    printf 'access_mode = "pairing"\n'
    printf 'poll_timeout_seconds = 30\n'
    echo
    echo "[codex]"
    printf 'bin = "codex"\n'
    echo
    echo "[state]"
    printf 'dir = "%s"\n' "$(toml_escape "${STATE_DIR}")"
    echo
    echo "[ui]"
    printf 'stream_edit_interval_ms = 1200\n'
    printf 'max_inline_diff_chars = 6000\n'
    echo
    echo "[git]"
    printf 'clone_timeout_sec = 600\n'
    printf 'allow_ssh = true\n'
    printf 'allow_https = true\n'
  } > "${CONFIG_TMP}"
  log "writing config template to ${CONFIG_PATH}"
  run_privileged install -m 0640 -o "${RUN_USER}" -g "${RUN_GROUP}" "${CONFIG_TMP}" "${CONFIG_PATH}"
else
  log "config already exists at ${CONFIG_PATH}, leaving it unchanged"
fi

if [[ ! -f "${ENV_PATH}" ]]; then
  {
    echo "# MyCodex environment"
    echo "# OPENAI_API_KEY=replace-me"
  } > "${ENV_TMP}"
  log "writing env template to ${ENV_PATH}"
  run_privileged install -m 0600 -o "${RUN_USER}" -g "${RUN_GROUP}" "${ENV_TMP}" "${ENV_PATH}"
else
  log "env file already exists at ${ENV_PATH}, leaving it unchanged"
fi

if [[ "${INSTALL_SYSTEMD}" == "true" ]]; then
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
  log "installing systemd service file to ${SERVICE_PATH}"
  run_privileged install -d -m 0755 "$(dirname "${SERVICE_PATH}")"
  run_privileged install -m 0644 "${SERVICE_TMP}" "${SERVICE_PATH}"
  if command -v systemctl >/dev/null 2>&1; then
    log "reloading systemd"
    run_privileged systemctl daemon-reload
  fi
fi

SERVICE_NAME="$(basename "${SERVICE_PATH}")"
if [[ "${UPDATE_MODE}" == "true" && "${INSTALL_SYSTEMD}" == "true" && -f "${SERVICE_PATH}" ]]; then
  if command -v systemctl >/dev/null 2>&1; then
    if run_privileged systemctl is-active --quiet "${SERVICE_NAME}"; then
      log "restarting active service ${SERVICE_NAME}"
      run_privileged systemctl restart "${SERVICE_NAME}"
    else
      log "service ${SERVICE_NAME} is installed but not active; leaving it stopped"
    fi
  fi
fi

cat <<EOF

MyCodex release $( [[ "${UPDATE_MODE}" == "true" ]] && printf 'update' || printf 'installation' ) complete.

Installed:
  binary:           ${INSTALL_BIN}
  config:           ${CONFIG_PATH}
  env file:         ${ENV_PATH}
  workspace root:   ${WORKSPACE_ROOT}
  state dir:        ${STATE_DIR}
  systemd service:  ${INSTALL_SYSTEMD}
  asset url:        ${ASSET_URL}

Next step:
  mycodex onboard --config ${CONFIG_PATH} --env-path ${ENV_PATH} --service-path ${SERVICE_PATH}
EOF
