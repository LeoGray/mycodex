#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

TARGET_TRIPLE=""
OUTPUT_DIR="${REPO_ROOT}/dist"
BINARY_NAME="mycodex"
BUILD_BINARY=1

usage() {
  cat <<'EOF'
Usage:
  ./scripts/package-release.sh [options]

Options:
  --target TARGET_TRIPLE   Rust target triple to build/package
  --output-dir PATH        Output directory for .tar.gz archives
  --binary-name NAME       Binary name inside the archive (default: mycodex)
  --skip-build             Reuse an existing release binary
  -h, --help

Examples:
  ./scripts/package-release.sh --target x86_64-unknown-linux-gnu
  ./scripts/package-release.sh --target aarch64-unknown-linux-musl
EOF
}

log() {
  printf '[mycodex-package] %s\n' "$*"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --target)
      TARGET_TRIPLE="$2"
      shift 2
      ;;
    --output-dir)
      OUTPUT_DIR="$2"
      shift 2
      ;;
    --binary-name)
      BINARY_NAME="$2"
      shift 2
      ;;
    --skip-build)
      BUILD_BINARY=0
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf '[mycodex-package] ERROR: unknown option: %s\n' "$1" >&2
      exit 1
      ;;
  esac
done

command -v cargo >/dev/null 2>&1 || {
  printf '[mycodex-package] ERROR: cargo is required\n' >&2
  exit 1
}
command -v tar >/dev/null 2>&1 || {
  printf '[mycodex-package] ERROR: tar is required\n' >&2
  exit 1
}

if [[ -z "${TARGET_TRIPLE}" ]]; then
  TARGET_TRIPLE="$(rustc -vV | awk '/^host:/ {print $2}')"
fi

if [[ "${BUILD_BINARY}" -eq 1 ]]; then
  log "building release binary for ${TARGET_TRIPLE}"
  cargo build --release --target "${TARGET_TRIPLE}" --manifest-path "${REPO_ROOT}/Cargo.toml"
fi

BINARY_PATH="${REPO_ROOT}/target/${TARGET_TRIPLE}/release/${BINARY_NAME}"
[[ -x "${BINARY_PATH}" ]] || {
  printf '[mycodex-package] ERROR: binary not found: %s\n' "${BINARY_PATH}" >&2
  exit 1
}

mkdir -p "${OUTPUT_DIR}"
STAGING_DIR="$(mktemp -d)"
trap 'rm -rf "${STAGING_DIR}"' EXIT

cp "${BINARY_PATH}" "${STAGING_DIR}/${BINARY_NAME}"
ARCHIVE_PATH="${OUTPUT_DIR}/${BINARY_NAME}-${TARGET_TRIPLE}.tar.gz"

log "writing archive ${ARCHIVE_PATH}"
tar -C "${STAGING_DIR}" -czf "${ARCHIVE_PATH}" "${BINARY_NAME}"
log "done"
