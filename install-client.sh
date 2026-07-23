#!/usr/bin/env bash

set -Eeuo pipefail

if [[ -r /etc/profile ]]; then
  # Some distributions return a non-zero status from optional profile hooks.
  source /etc/profile || true
fi

readonly BINARY="cc-switch-server"
readonly INSTALL_PATH="/usr/local/bin/${BINARY}"
readonly GITHUB_PROXY="https://gh-proxy.org"

PASSWORD=""
PROVISION_TOKEN=""
DOWNLOAD_TMP=""

cleanup_sensitive_state() {
  PASSWORD=""
  PROVISION_TOKEN=""
  unset PASSWORD PROVISION_TOKEN
  if [[ -n "${DOWNLOAD_TMP}" ]]; then
    rm -f -- "${DOWNLOAD_TMP}"
  fi
}
trap cleanup_sensitive_state EXIT

log() {
  printf '\033[44;37m[%s]\033[0m %s\n' "$(TZ=Asia/Shanghai date '+%Y-%m-%d %H:%M:%S')" "$1"
}

warn() {
  printf '\033[44;37m[%s]\033[0m \033[33m%s\033[0m\n' "$(TZ=Asia/Shanghai date '+%Y-%m-%d %H:%M:%S')" "$1"
}

die() {
  printf '\033[41;37m[%s]\033[0m %s\n' "$(TZ=Asia/Shanghai date '+%Y-%m-%d %H:%M:%S')" "$1" >&2
  exit 1
}

usage() {
  warn "Usage: install-client.sh ROUTER_URL OWNER_EMAIL --password-stdin [disableWebTerminal]"
  warn "       install-client.sh --provision-token-stdin ROUTER_URL [disableWebTerminal]"
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || die "Required command not found: $1"
}

normalize_router_url() {
  local value="${1%/}"
  [[ "${value}" =~ ^https?://[^/[:space:]]+$ ]] || die "ROUTER_URL must be an http(s) origin"
  printf '%s' "${value}"
}

fetch_provision_credentials() {
  local token="$1"
  [[ "${token}" =~ ^[A-Za-z0-9_-]{32,256}$ ]] || die "Invalid provision token format"
  require_command python3

  local response
  local -a address_family=()
  case "${CC_SWITCH_PROVISION_IP_FAMILY:-}" in
    4) address_family=(-4) ;;
    6) address_family=(-6) ;;
  esac
  response=$(printf '{"token":"%s"}' "${token}" | \
    curl "${address_family[@]}" --fail --silent --show-error \
      --retry 3 --retry-all-errors --connect-timeout 15 --max-time 60 \
      -H 'content-type: application/json' \
      -H 'accept: application/json' \
      --data-binary @- \
      "${ROUTER}/v1/client-market/provision-tokens/redeem") \
    || die "Provision token redemption failed"

  local -a credentials=()
  mapfile -d '' -t credentials < <(
    python3 -c '
import json, sys
try:
    data = json.load(sys.stdin)
    values = [data["routerUrl"], data["ownerEmail"], data["password"], data["subdomain"]]
    if not all(isinstance(value, str) and value for value in values):
        raise ValueError("missing credential")
except Exception:
    sys.exit(2)
for value in values:
    sys.stdout.buffer.write(value.encode("utf-8") + b"\0")
' <<<"${response}"
  )
  response=""
  [[ "${#credentials[@]}" -eq 4 ]] || die "Invalid provision token response"
  ROUTER=$(normalize_router_url "${credentials[0]}")
  OWNER="${credentials[1]}"
  PASSWORD="${credentials[2]}"
  CLIENT_SUBDOMAIN="${credentials[3]}"
  [[ "${OWNER}" =~ ^[^[:space:]@]+@[^[:space:]@]+\.[^[:space:]@]+$ ]] \
    || die "Invalid owner email in provision response"
  [[ "${CLIENT_SUBDOMAIN}" =~ ^[a-z0-9][a-z0-9-]{0,62}$ ]] \
    || die "Invalid client subdomain in provision response"
}

disable_web_terminal() {
  local config_path="${HOME}/.cc-switch-server/server.json"
  [[ -f "${config_path}" ]] || die "server.json was not created"
  require_command python3
  python3 - "${config_path}" <<'PY'
import json, os, sys, tempfile

path = sys.argv[1]
with open(path, "r", encoding="utf-8") as source:
    data = json.load(source)
data["enableWebTerminal"] = False
directory = os.path.dirname(path)
fd, temporary = tempfile.mkstemp(prefix="server.json.", dir=directory)
try:
    with os.fdopen(fd, "w", encoding="utf-8") as target:
        json.dump(data, target, indent=2, ensure_ascii=False)
        target.write("\n")
    os.replace(temporary, path)
finally:
    if os.path.exists(temporary):
        os.unlink(temporary)
PY
}

detect_download_url() {
  local asset
  case "$(uname -m)" in
    x86_64|amd64) asset="cc-switch-server-linux-amd64" ;;
    aarch64|arm64) asset="cc-switch-server-linux-arm64" ;;
    *) die "Unsupported architecture: $(uname -m)" ;;
  esac
  local url="https://github.com/Xiechengqi/cc-switch-server/releases/download/latest/${asset}"
  # Keep the existing mainland fallback, but do not make installation depend on ping.
  if curl --silent --max-time 3 http://3.0.3.0 2>/dev/null | grep -q '中国'; then
    url="${GITHUB_PROXY}/${url}"
  fi
  printf '%s' "${url}"
}

main() {
  [[ "${EUID}" -eq 0 ]] || die "Run this installer as root"
  require_command curl
  require_command install
  require_command mktemp
  require_command pgrep

  local provision_mode=0
  local disable_terminal=0
  CLIENT_SUBDOMAIN=""
  OWNER=""
  ROUTER=""

  case "${1:-}" in
    --provision-token-stdin)
      [[ "$#" -ge 2 && "$#" -le 3 ]] || { usage; exit 1; }
      [[ "$#" -lt 3 || "${3}" == "disableWebTerminal" ]] \
        || die "Unknown provisioning option: ${3}"
      provision_mode=1
      IFS= read -r PROVISION_TOKEN || die "Provision token was not provided on stdin"
      ROUTER=$(normalize_router_url "${2:-}")
      [[ "${3:-}" == "disableWebTerminal" ]] && disable_terminal=1
      fetch_provision_credentials "${PROVISION_TOKEN}"
      PROVISION_TOKEN=""
      ;;
    *)
      [[ "$#" -ge 3 && "$#" -le 4 ]] || { usage; exit 1; }
      [[ "$#" -lt 4 || "${4}" == "disableWebTerminal" ]] \
        || die "Unknown installation option: ${4}"
      ROUTER=$(normalize_router_url "$1")
      OWNER="$2"
      [[ "${OWNER}" =~ ^[^[:space:]@]+@[^[:space:]@]+\.[^[:space:]@]+$ ]] \
        || die "Invalid owner email"
      [[ "${3:-}" == "--password-stdin" ]] \
        || die "Client Web password must be provided with --password-stdin"
      IFS= read -r PASSWORD || die "Password was not provided on stdin"
      [[ "${4:-}" == "disableWebTerminal" ]] && disable_terminal=1
      ;;
  esac

  [[ "${#PASSWORD}" -ge 8 ]] || die "Client Web password must contain at least 8 characters"
  if pgrep -f '^/usr/local/bin/cc-switch-server( |$)' >/dev/null 2>&1; then
    if [[ "${provision_mode}" -eq 1 ]]; then
      die "Provisioning requires a clean host"
    fi
    warn "cc-switch-server is already running; stop it before reinstalling"
    exit 0
  fi
  if [[ "${provision_mode}" -eq 1 ]] && {
    [[ -e "${INSTALL_PATH}" || -e "${HOME}/.cc-switch-server" ]] \
      || compgen -G "${HOME}/.cc-switch-server.bak.*" >/dev/null;
  }; then
    die "Provisioning requires a clean host"
  fi

  local download_url
  download_url=$(detect_download_url)
  DOWNLOAD_TMP=$(mktemp /tmp/cc-switch-server.XXXXXX)
  log "Downloading cc-switch-server"
  curl --fail --silent --show-error --location \
    --retry 3 --retry-all-errors --connect-timeout 15 --max-time 300 \
    "${download_url}" -o "${DOWNLOAD_TMP}" || die "Binary download failed"
  chmod 0755 "${DOWNLOAD_TMP}"
  "${DOWNLOAD_TMP}" -V >/dev/null || die "Downloaded binary validation failed"
  install -m 0755 "${DOWNLOAD_TMP}" "${INSTALL_PATH}"

  if [[ "${provision_mode}" -eq 0 && -e "${HOME}/.cc-switch-server" ]]; then
    local backup_path="${HOME}/.cc-switch-server.bak.$(date +%s).$$"
    log "Backing up existing Client data to ${backup_path}"
    mv -- "${HOME}/.cc-switch-server" "${backup_path}"
  fi

  log "Initializing Client for ${OWNER}"
  local -a init_args=(
    init --router-url "${ROUTER}" --owner-email "${OWNER}" --password-stdin
  )
  if [[ -n "${CLIENT_SUBDOMAIN}" ]]; then
    init_args+=(--client-subdomain "${CLIENT_SUBDOMAIN}")
  fi
  printf '%s\n' "${PASSWORD}" | "${INSTALL_PATH}" "${init_args[@]}" >/dev/null \
    || die "cc-switch-server init failed"
  PASSWORD=""

  if [[ "${disable_terminal}" -eq 1 ]]; then
    disable_web_terminal
  fi

  unset CC_SWITCH_PROVISION_IP_FAMILY
  log "Starting cc-switch-server"
  nohup "${INSTALL_PATH}" >/dev/null 2>&1 &
  local pid=$!
  sleep 3
  kill -0 "${pid}" 2>/dev/null || die "cc-switch-server did not remain running"

  local subdomain="${CLIENT_SUBDOMAIN}"
  if [[ -z "${subdomain}" ]] && command -v python3 >/dev/null 2>&1; then
    subdomain=$(python3 -c '
import json, os
with open(os.path.expanduser("~/.cc-switch-server/server.json"), encoding="utf-8") as source:
    print(json.load(source).get("tunnelSubdomain", ""))
' 2>/dev/null || true)
  fi
  if [[ -n "${subdomain}" ]]; then
    local authority="${ROUTER#*://}"
    local scheme="${ROUTER%%://*}"
    warn "Client is ready at ${scheme}://${subdomain}.${authority}"
  else
    warn "Client installation completed"
  fi
}

main "$@"
