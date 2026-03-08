#!/usr/bin/env bash
set -euo pipefail

DEFAULT_GITHUB_REPO="${MYCODEX_DEFAULT_GITHUB_REPO:-LeoGray/mycodex}"
RELEASE_VERSION="${MYCODEX_RELEASE_VERSION:-latest}"
RELEASE_ASSET_URL="${MYCODEX_RELEASE_ASSET_URL:-}"
RELEASE_TARGET_TRIPLE="${MYCODEX_RELEASE_TARGET_TRIPLE:-}"
RELEASE_BINARY_NAME="mycodex"
OFFICIAL_RELEASE_TARGET="x86_64-unknown-linux-musl"
OS_NAME="$(uname -s)"
SERVICE_LABEL="com.leogray.mycodex"

INSTALL_BIN=""
CONFIG_PATH=""
ENV_PATH=""
SERVICE_PATH=""
STATE_DIR=""
WORKSPACE_ROOT=""
WORKSPACE_ROOT_SET_BY_FLAG="false"
RUN_USER="${SUDO_USER:-$(id -un)}"
RUN_GROUP=""
RUN_HOME=""
INSTALL_SERVICE=""
UPDATE_MODE=""
AUTO_DETECTED_UPDATE="false"
SERVICE_MANAGER=""
LAUNCH_SCRIPT_PATH=""
GITHUB_REPO_OVERRIDE=""
TEMP_DIR=""
STATE_DIR_SET_BY_FLAG="false"

usage() {
  cat <<'EOF'
Usage:
  curl -fsSL https://raw.githubusercontent.com/LeoGray/mycodex/main/public/install.sh | bash
  curl -fsSL https://raw.githubusercontent.com/LeoGray/mycodex/main/public/install.sh | bash -s -- [options]

This installer downloads the official prebuilt MyCodex release for x86_64 Linux.
For macOS or other targets, provide a self-built archive with --asset-url
or use ./scripts/install.sh from a cloned repository.
Configuration happens later via:

  /path/to/mycodex onboard

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
  --install-service
  --skip-service
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

xml_escape() {
  printf '%s' "$1" | sed \
    -e 's/&/\&amp;/g' \
    -e 's/</\&lt;/g' \
    -e 's/>/\&gt;/g' \
    -e "s/'/\&apos;/g" \
    -e 's/"/\&quot;/g'
}

resolve_home_dir() {
  local user="$1"
  local home=""

  if command -v getent >/dev/null 2>&1; then
    home="$(getent passwd "${user}" | cut -d: -f6)"
  elif [[ "${OS_NAME}" == "Darwin" ]] && command -v dscl >/dev/null 2>&1; then
    home="$(dscl . -read "/Users/${user}" NFSHomeDirectory 2>/dev/null | awk '{print $2}')"
  fi

  if [[ -z "${home}" ]]; then
    home="$(eval "printf '%s' ~${user}")"
  fi

  printf '%s\n' "${home}"
}

read_toml_string() {
  local file="$1"
  local section="$2"
  local key="$3"

  awk -v section="${section}" -v key="${key}" '
    $0 ~ "^[[:space:]]*\\[" section "\\][[:space:]]*$" { in_section=1; next }
    in_section && $0 ~ "^[[:space:]]*\\[" { exit }
    in_section && $0 ~ "^[[:space:]]*" key "[[:space:]]*=" { print; exit }
  ' "${file}" | sed -E 's/^[^=]+=[[:space:]]*"//; s/"[[:space:]]*$//'
}

sync_existing_config_values() {
  if [[ ! -f "${CONFIG_PATH}" ]]; then
    return
  fi

  local configured_workspace=""
  local configured_state=""

  if [[ "${WORKSPACE_ROOT_SET_BY_FLAG}" != "true" ]]; then
    configured_workspace="$(read_toml_string "${CONFIG_PATH}" workspace root)"
    if [[ -n "${configured_workspace}" ]]; then
      WORKSPACE_ROOT="${configured_workspace}"
    fi
  fi

  if [[ "${STATE_DIR_SET_BY_FLAG}" != "true" ]]; then
    configured_state="$(read_toml_string "${CONFIG_PATH}" state dir)"
    if [[ -n "${configured_state}" ]]; then
      STATE_DIR="${configured_state}"
    fi
  fi
}

path_is_in_run_home() {
  local path="$1"
  [[ "${path}" == "${RUN_HOME}" || "${path}" == "${RUN_HOME}/"* ]]
}

install_dir() {
  local path="$1"
  local -a cmd=(install -d -m 0755)

  if path_is_in_run_home "${path}"; then
    cmd+=(-o "${RUN_USER}" -g "${RUN_GROUP}")
  fi

  run_privileged "${cmd[@]}" "${path}"
}

install_file() {
  local mode="$1"
  local src="$2"
  local dest="$3"
  local -a cmd=(install -m "${mode}")

  if path_is_in_run_home "${dest}"; then
    cmd+=(-o "${RUN_USER}" -g "${RUN_GROUP}")
  fi

  run_privileged "${cmd[@]}" "${src}" "${dest}"
}

service_install_prompt() {
  if [[ "${SERVICE_MANAGER}" == "systemd" ]]; then
    printf '%s\n' "Install a systemd service file?"
  else
    printf '%s\n' "Install a launchd agent plist?"
  fi
}

apply_platform_defaults() {
  case "${OS_NAME}" in
    Linux)
      SERVICE_MANAGER="systemd"
      : "${INSTALL_BIN:=/usr/local/bin/mycodex}"
      : "${CONFIG_PATH:=/etc/mycodex/config.toml}"
      : "${ENV_PATH:=/etc/mycodex/mycodex.env}"
      : "${SERVICE_PATH:=/etc/systemd/system/mycodex.service}"
      : "${STATE_DIR:=/var/lib/mycodex}"
      ;;
    Darwin)
      SERVICE_MANAGER="launchd"
      : "${INSTALL_BIN:=/usr/local/bin/mycodex}"
      : "${CONFIG_PATH:=${RUN_HOME}/.config/mycodex/config.toml}"
      : "${ENV_PATH:=${RUN_HOME}/.config/mycodex/mycodex.env}"
      : "${SERVICE_PATH:=${RUN_HOME}/Library/LaunchAgents/${SERVICE_LABEL}.plist}"
      : "${STATE_DIR:=${RUN_HOME}/.local/state/mycodex}"
      ;;
    *)
      die "unsupported operating system: ${OS_NAME}"
      ;;
  esac

  if [[ -z "${WORKSPACE_ROOT}" ]]; then
    WORKSPACE_ROOT="${RUN_HOME}/workspace"
  fi

  if [[ -z "${LAUNCH_SCRIPT_PATH}" && "${SERVICE_MANAGER}" == "launchd" ]]; then
    LAUNCH_SCRIPT_PATH="$(dirname "${ENV_PATH}")/launch-mycodex.sh"
  fi
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
    *) die "unsupported architecture: ${arch}" ;;
  esac

  case "${OS_NAME}" in
    Linux)
      local libc_suffix="unknown-linux-musl"
      if command -v ldd >/dev/null 2>&1; then
        if ldd --version 2>&1 | grep -qi 'musl'; then
          libc_suffix="unknown-linux-musl"
        else
          libc_suffix="unknown-linux-gnu"
        fi
      fi
      printf '%s-%s\n' "${arch}" "${libc_suffix}"
      ;;
    Darwin)
      printf '%s-apple-darwin\n' "${arch}"
      ;;
    *)
      die "unsupported operating system: ${OS_NAME}"
      ;;
  esac
}

resolve_release_asset_url() {
  if [[ -n "${RELEASE_ASSET_URL}" ]]; then
    printf '%s\n' "${RELEASE_ASSET_URL}"
    return
  fi

  local repo="${DEFAULT_GITHUB_REPO}"
  if [[ -n "${GITHUB_REPO_OVERRIDE}" ]]; then
    repo="${GITHUB_REPO_OVERRIDE}"
  fi

  local target
  if [[ -z "${GITHUB_REPO_OVERRIDE}" && -z "${RELEASE_TARGET_TRIPLE}" ]]; then
    local arch
    arch="$(uname -m)"
    case "${arch}" in
      x86_64|amd64) ;;
      *)
        die "official release assets are only published for ${OFFICIAL_RELEASE_TARGET}; build locally or pass --asset-url for your own archive"
        ;;
    esac

    if [[ "${OS_NAME}" != "Linux" ]]; then
      die "official release assets are only published for ${OFFICIAL_RELEASE_TARGET}; use ./scripts/install.sh on macOS or pass --asset-url for a self-built archive"
    fi

    target="${OFFICIAL_RELEASE_TARGET}"
  else
    target="$(detect_target_triple)"
  fi

  local asset_name="${RELEASE_BINARY_NAME}-${target}.tar.gz"

  if [[ "${RELEASE_VERSION}" == "latest" ]]; then
    printf 'https://github.com/%s/releases/latest/download/%s\n' "${repo}" "${asset_name}"
  else
    printf 'https://github.com/%s/releases/download/%s/%s\n' "${repo}" "${RELEASE_VERSION}" "${asset_name}"
  fi
}

launchd_agent_loaded() {
  printf -v cmd 'launchctl list | grep -Fq %q' "${SERVICE_LABEL}"
  run_as_user "${RUN_USER}" bash -lc "${cmd}"
}

reload_launchd_agent() {
  printf -v cmd 'launchctl unload %q >/dev/null 2>&1 || true; launchctl load -w %q' \
    "${SERVICE_PATH}" "${SERVICE_PATH}"
  run_as_user "${RUN_USER}" bash -lc "${cmd}"
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
      WORKSPACE_ROOT_SET_BY_FLAG="true"
      shift 2
      ;;
    --state-dir)
      STATE_DIR="$2"
      STATE_DIR_SET_BY_FLAG="true"
      shift 2
      ;;
    --install-bin)
      INSTALL_BIN="$2"
      shift 2
      ;;
    --config-path)
      CONFIG_PATH="$2"
      shift 2
      ;;
    --env-path)
      ENV_PATH="$2"
      shift 2
      ;;
    --service-path)
      SERVICE_PATH="$2"
      shift 2
      ;;
    --install-service|--install-systemd)
      INSTALL_SERVICE="true"
      shift
      ;;
    --skip-service|--skip-systemd)
      INSTALL_SERVICE="false"
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
RUN_HOME="$(resolve_home_dir "${RUN_USER}")"
[[ -n "${RUN_HOME}" ]] || die "failed to resolve home directory for ${RUN_USER}"

apply_platform_defaults
sync_existing_config_values

if [[ -z "${UPDATE_MODE}" ]]; then
  if [[ -x "${INSTALL_BIN}" || -f "${CONFIG_PATH}" || -f "${SERVICE_PATH}" ]]; then
    UPDATE_MODE="true"
    AUTO_DETECTED_UPDATE="true"
    log "detected existing installation; using update mode"
  else
    UPDATE_MODE="false"
  fi
fi

if [[ -z "${INSTALL_SERVICE}" ]]; then
  if [[ "${UPDATE_MODE}" == "true" ]]; then
    if [[ -f "${SERVICE_PATH}" ]]; then
      INSTALL_SERVICE="true"
    else
      INSTALL_SERVICE="false"
    fi
  else
    if confirm "$(service_install_prompt)" true; then
      INSTALL_SERVICE="true"
    else
      INSTALL_SERVICE="false"
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
install_dir "$(dirname "${INSTALL_BIN}")"
install_file 0755 "${BINARY_PATH}" "${INSTALL_BIN}"

log "preparing directories"
install_dir "$(dirname "${CONFIG_PATH}")"
install_dir "$(dirname "${ENV_PATH}")"
install_dir "${STATE_DIR}"
install_dir "${WORKSPACE_ROOT}"

CONFIG_TMP="$(mktemp)"
ENV_TMP="$(mktemp)"
SERVICE_TMP="$(mktemp)"
LAUNCH_SCRIPT_TMP="$(mktemp)"
trap 'rm -f "${CONFIG_TMP}" "${ENV_TMP}" "${SERVICE_TMP}" "${LAUNCH_SCRIPT_TMP}"; cleanup' EXIT

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
    printf 'network_access = true\n'
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
  install_file 0640 "${CONFIG_TMP}" "${CONFIG_PATH}"
else
  log "config already exists at ${CONFIG_PATH}, leaving it unchanged"
fi

if [[ ! -f "${ENV_PATH}" ]]; then
  {
    echo "# MyCodex environment"
    echo "# OPENAI_API_KEY=replace-me"
  } > "${ENV_TMP}"
  log "writing env template to ${ENV_PATH}"
  install_file 0600 "${ENV_TMP}" "${ENV_PATH}"
else
  log "env file already exists at ${ENV_PATH}, leaving it unchanged"
fi

if [[ "${INSTALL_SERVICE}" == "true" ]]; then
  install_dir "$(dirname "${SERVICE_PATH}")"

  if [[ "${SERVICE_MANAGER}" == "systemd" ]]; then
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
    install_file 0644 "${SERVICE_TMP}" "${SERVICE_PATH}"
    if command -v systemctl >/dev/null 2>&1; then
      log "reloading systemd"
      run_privileged systemctl daemon-reload
    fi
  else
    printf -v install_bin_escaped '%q' "${INSTALL_BIN}"
    printf -v config_path_escaped '%q' "${CONFIG_PATH}"
    printf -v env_path_escaped '%q' "${ENV_PATH}"

    {
      echo "#!/usr/bin/env bash"
      echo "set -euo pipefail"
      echo 'export RUST_LOG="${RUST_LOG:-info}"'
      printf 'if [[ -f %s ]]; then\n' "${env_path_escaped}"
      echo "  set -a"
      echo "  # shellcheck disable=SC1090"
      printf '  . %s\n' "${env_path_escaped}"
      echo "  set +a"
      echo "fi"
      printf 'exec %s serve --config %s\n' "${install_bin_escaped}" "${config_path_escaped}"
    } > "${LAUNCH_SCRIPT_TMP}"

    install_dir "$(dirname "${LAUNCH_SCRIPT_PATH}")"
    log "installing launch script to ${LAUNCH_SCRIPT_PATH}"
    install_file 0755 "${LAUNCH_SCRIPT_TMP}" "${LAUNCH_SCRIPT_PATH}"

    {
      cat <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>$(xml_escape "${SERVICE_LABEL}")</string>
  <key>ProgramArguments</key>
  <array>
    <string>$(xml_escape "${LAUNCH_SCRIPT_PATH}")</string>
  </array>
  <key>WorkingDirectory</key>
  <string>$(xml_escape "${STATE_DIR}")</string>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>$(xml_escape "${STATE_DIR}/mycodex.stdout.log")</string>
  <key>StandardErrorPath</key>
  <string>$(xml_escape "${STATE_DIR}/mycodex.stderr.log")</string>
</dict>
</plist>
EOF
    } > "${SERVICE_TMP}"

    log "installing launchd agent plist to ${SERVICE_PATH}"
    install_file 0644 "${SERVICE_TMP}" "${SERVICE_PATH}"
  fi
fi

if [[ "${UPDATE_MODE}" == "true" && "${INSTALL_SERVICE}" == "true" && -f "${SERVICE_PATH}" ]]; then
  if [[ "${SERVICE_MANAGER}" == "systemd" ]]; then
    SERVICE_NAME="$(basename "${SERVICE_PATH}")"
    if command -v systemctl >/dev/null 2>&1; then
      if run_privileged systemctl is-active --quiet "${SERVICE_NAME}"; then
        log "restarting active service ${SERVICE_NAME}"
        run_privileged systemctl restart "${SERVICE_NAME}"
      else
        log "service ${SERVICE_NAME} is installed but not active; leaving it stopped"
      fi
    fi
  else
    if launchd_agent_loaded; then
      log "reloading active launchd agent ${SERVICE_LABEL}"
      reload_launchd_agent
    else
      log "launchd agent ${SERVICE_LABEL} is installed but not loaded; leaving it stopped"
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
  service manager:  ${SERVICE_MANAGER}
  service file:     $( [[ "${INSTALL_SERVICE}" == "true" ]] && printf '%s' "${SERVICE_PATH}" || printf 'not installed' )
  asset url:        ${ASSET_URL}
EOF

if [[ "${SERVICE_MANAGER}" == "launchd" && "${INSTALL_SERVICE}" == "true" ]]; then
  printf '  launch script:    %s\n' "${LAUNCH_SCRIPT_PATH}"
fi

if [[ "${UPDATE_MODE}" == "true" ]]; then
  cat <<EOF

No onboarding step is required for this update.
Optional reconfiguration:
  ${INSTALL_BIN} onboard --config ${CONFIG_PATH} --env-path ${ENV_PATH} --service-path ${SERVICE_PATH}
EOF
else
  cat <<EOF

Next step:
  ${INSTALL_BIN} onboard --config ${CONFIG_PATH} --env-path ${ENV_PATH} --service-path ${SERVICE_PATH}
EOF
fi
