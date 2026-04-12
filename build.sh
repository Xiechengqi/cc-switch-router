#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "${ROOT_DIR}"

TARGET_TRIPLE="${TARGET_TRIPLE:-}"
BUILD_MODE="${BUILD_MODE:-release}"
DIST_DIR="${ROOT_DIR}/dist"
BIN_NAME="portr-rs"

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo 未安装，无法构建 ${BIN_NAME}" >&2
  exit 1
fi

mkdir -p "${DIST_DIR}"

BUILD_ARGS=()
if [ "${BUILD_MODE}" = "release" ]; then
  BUILD_ARGS+=(--release)
fi

if [ -n "${TARGET_TRIPLE}" ]; then
  BUILD_ARGS+=(--target "${TARGET_TRIPLE}")
fi

echo "==> 构建 ${BIN_NAME} (${BUILD_MODE})"
cargo build "${BUILD_ARGS[@]}"

if [ -n "${TARGET_TRIPLE}" ]; then
  TARGET_DIR="${ROOT_DIR}/target/${TARGET_TRIPLE}/${BUILD_MODE}"
else
  TARGET_DIR="${ROOT_DIR}/target/${BUILD_MODE}"
fi

BIN_PATH="${TARGET_DIR}/${BIN_NAME}"
if [ ! -f "${BIN_PATH}" ]; then
  echo "构建完成，但未找到产物: ${BIN_PATH}" >&2
  exit 1
fi

VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)"
ARCHIVE_BASENAME="${BIN_NAME}_${VERSION}"
if [ -n "${TARGET_TRIPLE}" ]; then
  ARCHIVE_BASENAME="${ARCHIVE_BASENAME}_${TARGET_TRIPLE}"
fi

STAGE_DIR="${DIST_DIR}/${ARCHIVE_BASENAME}"
ARCHIVE_PATH="${DIST_DIR}/${ARCHIVE_BASENAME}.tar.gz"
CHECKSUM_PATH="${ARCHIVE_PATH}.sha256"

rm -rf "${STAGE_DIR}" "${ARCHIVE_PATH}" "${CHECKSUM_PATH}"
mkdir -p "${STAGE_DIR}"

cp "${BIN_PATH}" "${STAGE_DIR}/${BIN_NAME}"
cp README.md "${STAGE_DIR}/README.md"
if [ -f ARCHITECTURE.md ]; then
  cp ARCHITECTURE.md "${STAGE_DIR}/ARCHITECTURE.md"
fi

cat > "${STAGE_DIR}/portr-rs.env.example" <<'EOF'
PORTR_RS_API_ADDR=0.0.0.0:8787
PORTR_RS_SSH_ADDR=0.0.0.0:2222
PORTR_RS_TUNNEL_DOMAIN=example.com
PORTR_RS_USE_LOCALHOST=false
PORTR_RS_LEASE_TTL_SECS=60
PORTR_RS_DB_PATH=$HOME/.config/portr-rs/portr-rs.db
PORTR_RS_ADMIN_TOKEN=change-me-admin-token
PORTR_RS_CLEANUP_INTERVAL_SECS=300
PORTR_RS_LEASE_RETENTION_SECS=604800
EOF

tar -C "${DIST_DIR}" -czf "${ARCHIVE_PATH}" "${ARCHIVE_BASENAME}"

if command -v sha256sum >/dev/null 2>&1; then
  sha256sum "${ARCHIVE_PATH}" > "${CHECKSUM_PATH}"
elif command -v shasum >/dev/null 2>&1; then
  shasum -a 256 "${ARCHIVE_PATH}" > "${CHECKSUM_PATH}"
fi

echo "==> 构建完成"
echo "binary: ${BIN_PATH}"
echo "archive: ${ARCHIVE_PATH}"
if [ -f "${CHECKSUM_PATH}" ]; then
  echo "sha256: ${CHECKSUM_PATH}"
fi
