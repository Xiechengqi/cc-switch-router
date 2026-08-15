#!/usr/bin/env bash

#
# 2026/07/13
# install cc-switch-server
#

[ -r /etc/profile ] && . /etc/profile || true

# println information
INFO() {
printf -- "\033[44;37m%s\033[0m " "[$(TZ=UTC-8 date "+%Y-%m-%d %H:%M:%S")]"
printf -- "%s" "$1"
printf "\n"
}

# println yellow color information
YELLOW() {
printf -- "\033[44;37m%s\033[0m " "[$(TZ=UTC-8 date "+%Y-%m-%d %H:%M:%S")]"
printf -- "\033[33m%s\033[0m" "$1"
printf "\n"
}

# println error information
ERROR() {
printf -- "\033[41;37m%s\033[0m " "[$(TZ=UTC-8 date "+%Y-%m-%d %H:%M:%S")]"
printf -- "%s" "$1"
printf "\n"
exit 1
}

# exec cmd and print error information
EXEC() {
local cmd="$1"
INFO "${cmd}"
eval ${cmd} 1> /dev/null
if [ $? -ne 0 ]; then
ERROR "Execution command (${cmd}) failed, please check it and try again."
fi
}

check_if_in_china() {

if ! hash ping || ! hash curl
then
echo "Install ping and curl first please" && exit 1
fi
! ping -c 3 -W 3 1.1.1.1 &> /dev/null && ! ping -c 3 -W 3 baidu.com &> /dev/null && echo "Network exception, unable to connect to Ethernet !!!" && exit 1
check_location_api_arr=("3.0.3.0" "3.0.2.1" "3.0.2.9")
for check_location_api in ${check_location_api_arr[*]}
do
location=$(timeout 3 curl -SsL ${check_location_api} | grep location)
[ ".${location}" = "." ] && continue
echo "${location}" | grep -E -v '香港|澳门|台湾' | grep '中国' &> /dev/null && echo "China" || echo "Other"
break
done

}

USAGE() {
YELLOW "Usage: printf '%s\\n' \"\${WEB_PASSWORD}\" | bash install-client.sh [Router_Url] [Owner_Email] --password-stdin [Client_Subdomain] [disableWebTerminal]"
YELLOW "       Legacy mode accepts the password as argument 3 but exposes it through process arguments and shell history."
YELLOW "       Client_Subdomain and disableWebTerminal are optional; disableWebTerminal disables the Client web terminal."
}

export GITHUB_PROXY="https://gh-proxy.org"

disable_web_terminal() {
  local conf="${HOME}/.cc-switch-server/server.json"
  [ -f "${conf}" ] || ERROR "server.json was not created"
  if grep -q 'enableWebTerminal' "${conf}"; then
    EXEC "sed -i 's/\"enableWebTerminal\"[[:space:]]*:[[:space:]]*true/\"enableWebTerminal\": false/g' ${conf}"
  else
    YELLOW "enableWebTerminal not found in server.json; skip disabling"
  fi
}

install_systemd_service() {
  command -v systemctl >/dev/null 2>&1 || return 1
  [ -d /run/systemd/system ] || return 1
  INFO "Installing systemd service for cc-switch-server"
  local service_file="/etc/systemd/system/cc-switch-server.service"
  local service_tmp
  service_tmp=$(mktemp) || ERROR "Unable to create systemd service temporary file"
  cat > "${service_tmp}" <<'EOF'
[Unit]
Description=cc-switch-server
Wants=network-online.target
After=network-online.target

[Service]
Type=simple
User=root
Environment=HOME=/root
WorkingDirectory=/root
ExecStart=/usr/local/bin/cc-switch-server
Restart=no
EOF
  install -m 0644 "${service_tmp}" "${service_file}" \
    || { rm -f "${service_tmp}"; ERROR "Unable to install systemd service"; }
  rm -f "${service_tmp}"
  systemctl daemon-reload || ERROR "Unable to reload systemd"
  systemctl disable cc-switch-server.service >/dev/null 2>&1 || true
  systemctl reset-failed cc-switch-server.service >/dev/null 2>&1 || true
  systemctl start cc-switch-server.service || ERROR "Unable to start cc-switch-server"
  SERVICE_MANAGER="systemd"
  return 0
}

install_openrc_service() {
  command -v rc-service >/dev/null 2>&1 || return 1
  command -v rc-update >/dev/null 2>&1 || return 1
  [ -d /etc/init.d ] || return 1
  INFO "Installing OpenRC service for cc-switch-server"
  local service_file="/etc/init.d/cc-switch-server"
  local service_tmp
  service_tmp=$(mktemp) || ERROR "Unable to create OpenRC service temporary file"
  cat > "${service_tmp}" <<'EOF'
#!/sbin/openrc-run

name="cc-switch-server"
description="cc-switch-server"
command="/usr/local/bin/cc-switch-server"
command_user="root:root"
directory="/root"
command_background=true
pidfile="/run/cc-switch-server.pid"

depend() {
  need net
  after firewall
}
EOF
  install -m 0755 "${service_tmp}" "${service_file}" \
    || { rm -f "${service_tmp}"; ERROR "Unable to install OpenRC service"; }
  rm -f "${service_tmp}"
  rc-update del cc-switch-server default >/dev/null 2>&1 || true
  rc-service cc-switch-server start || ERROR "Unable to start cc-switch-server OpenRC service"
  SERVICE_MANAGER="openrc"
  return 0
}

start_cc_switch_server() {
  SERVICE_MANAGER="nohup"
  if install_systemd_service; then
    return 0
  fi
  if install_openrc_service; then
    return 0
  fi
  YELLOW "No supported service manager found; using nohup fallback"
  EXEC "nohup /usr/local/bin/${binary} </dev/null &> /dev/null &"
}

main() {

# check location
countryCode=$(check_if_in_china)
[ ".${countryCode}" = "." ] && ERROR "Get country location fail ..."
INFO "Location: ${countryCode}"

# environment
! uname -m | grep -E 'aarch|arm' &> /dev/null && downloadUrl="https://github.com/Xiechengqi/cc-switch-server/releases/download/latest/cc-switch-server-linux-amd64" || downloadUrl="https://github.com/Xiechengqi/cc-switch-server/releases/download/latest/cc-switch-server-linux-arm64"
[ "${countryCode}" = "China" ] && downloadUrl="${GITHUB_PROXY}/${downloadUrl}"
binary="cc-switch-server"

# check process (exact name; avoid matching this script's own argv/cmdline)
if { command -v pgrep >/dev/null 2>&1 && pgrep -x "${binary}" >/dev/null 2>&1; } \
  || { command -v pidof >/dev/null 2>&1 && pidof "${binary}" >/dev/null 2>&1; }
then
YELLOW "${binary} is running, exit installing ..."
command -v pgrep >/dev/null 2>&1 && pgrep -ax "${binary}" 2>/dev/null || ps -ef | grep "[c]c-switch-server" || true
exit 0
fi

ROUTER=${1} && [ ".${ROUTER}" = "." ] && USAGE && exit 1
ROUTER=$(echo ${ROUTER} | sed 's/\/$//')
echo ${ROUTER} | grep -E '^https://|^http://' &> /dev/null || ERROR "ROUTER must be like https://xxx or http://xxx"
OWNER=${2} && [ ".${OWNER}" = "." ] && USAGE && exit 1
PASSWORD_ARG=${3} && [ ".${PASSWORD_ARG}" = "." ] && USAGE && exit 1
PASSWORD_STDIN=0
if [ "${PASSWORD_ARG}" = "--password-stdin" ]; then
  PASSWORD_STDIN=1
  IFS= read -r PASSWORD || ERROR "Unable to read Client Web password from stdin"
else
  PASSWORD="${PASSWORD_ARG}"
fi
[ ".${PASSWORD}" = "." ] && ERROR "Client Web password must not be empty"

CLIENT_SUBDOMAIN=""
DISABLE_WEB_TERMINAL=0
shift 3 || true
for arg in "$@"; do
  if [ "${arg}" = "disableWebTerminal" ]; then
    DISABLE_WEB_TERMINAL=1
  elif [ ".${CLIENT_SUBDOMAIN}" = "." ]; then
    CLIENT_SUBDOMAIN="${arg}"
  else
    ERROR "Unknown argument: ${arg}"
  fi
done
if [ ".${CLIENT_SUBDOMAIN}" != "." ]; then
  echo "${CLIENT_SUBDOMAIN}" | grep -E '^[a-z0-9][a-z0-9-]{0,62}$' &> /dev/null \
    || ERROR "Client subdomain must be lowercase letters, digits, and hyphens"
fi

mkdir -p /usr/local/bin

# download tarball
EXEC "curl -SsL ${downloadUrl} -o /usr/local/bin/${binary} && chmod +x /usr/local/bin/${binary}"
EXEC "${binary} -V" && ${binary} -V

# start
INFO "Client Owner Email: ${OWNER}"
INFO "Client Web Password: configured"
INFO "Client Register To Router: ${ROUTER}"
[ ".${CLIENT_SUBDOMAIN}" != "." ] && INFO "Client Subdomain: ${CLIENT_SUBDOMAIN}"
[ "${DISABLE_WEB_TERMINAL}" -eq 1 ] && INFO "Web Terminal: disabled"
YELLOW "Check whether the above parameters are correct and start the installation in 3 seconds ..." && EXEC "sleep 3"

cd $HOME &> /dev/null && ls .cc-switch-server &> /dev/null && INFO "Backup old local data ..." && EXEC "mv -v .cc-switch-server .cc-switch-server.bak.$(date +%s)"
INIT_ARGS=(init --router-url "${ROUTER}" --owner-email "${OWNER}")
[ ".${CLIENT_SUBDOMAIN}" != "." ] && INIT_ARGS+=(--client-subdomain "${CLIENT_SUBDOMAIN}")
if [ "${PASSWORD_STDIN}" -eq 1 ]; then
  printf '%s\n' "${PASSWORD}" | /usr/local/bin/${binary} "${INIT_ARGS[@]}" --password-stdin \
    || ERROR "cc-switch-server init failed"
else
  /usr/local/bin/${binary} "${INIT_ARGS[@]}" --password "${PASSWORD}" \
    || ERROR "cc-switch-server init failed"
fi
unset PASSWORD PASSWORD_ARG
[ "${DISABLE_WEB_TERMINAL}" -eq 1 ] && disable_web_terminal
start_cc_switch_server
EXEC "sleep 3"
INFO "Client process manager: ${SERVICE_MANAGER} (manual restart policy)"
SUBDOMAIN=$(grep tunnelSubdomain $HOME/.cc-switch-server/server.json  | awk -F '"' '{print $(NF-1)}')
SUBDOMAIN_URL=$(echo ${ROUTER} | sed "s/:\/\//:\/\/${SUBDOMAIN}\./")

# info
YELLOW "Please visit ${SUBDOMAIN_URL} with the browser ..."
YELLOW "Web Password: configured during setup (not displayed)"

}

main "$@"
