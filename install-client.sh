#!/usr/bin/env bash

#
# 2026/07/13
# install cc-switch-server
#

source /etc/profile

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
YELLOW "Usage: curl -SsL https://[Router]/install-client.sh | bash -s [Router_Url] [Owner_Email] [Web_Login_Password]"
}

export GITHUB_PROXY="https://gh-proxy.org"

main() {

# check location
countryCode=$(check_if_in_china)
[ ".${countryCode}" = "." ] && ERROR "Get country location fail ..."
INFO "Location: ${countryCode}"

# environment
! uname -m | grep -E 'aarch|arm' &> /dev/null && downloadUrl="https://github.com/Xiechengqi/cc-switch-server/releases/download/latest/cc-switch-server-linux-amd64" || downloadUrl="https://github.com/Xiechengqi/cc-switch-server/releases/download/latest/cc-switch-server-linux-arm64"
[ "${countryCode}" = "China" ] && downloadUrl="${GITHUB_PROXY}/${downloadUrl}"
binary="cc-switch-server"

# check process
if ps -ef | grep ${binary} | grep -v grep &> /dev/null
then
YELLOW "${binary} is running, exit installing ..." && ps -ef | grep ${binary} | grep -v grep
exit 0
fi

ROUTER=${1} && [ ".${ROUTER}" = "." ] && USAGE
ROUTER=$(echo ${ROUTER} | sed 's/\/$//')
echo ${ROUTER} | grep -E '^https://|^http://' &> /dev/null || ERROR "ROUTER must be like https://xxx or http://xxx"
OWNER=${2} && [ ".${OWNER}" = "." ] && USAGE
PASSWORD=${3} && [ ".${PASSWORD}" = "." ] && USAGE

# download tarball
EXEC "curl -SsL ${downloadUrl} -o /usr/local/bin/${binary} && chmod +x /usr/local/bin/${binary}"
EXEC "${binary} -V" && ${binary} -V

# start
INFO "Client Owner Email: ${OWNER}"
INFO "Client Web Password: ${PASSWORD}"
INFO "Client Register To Router: ${ROUTER}"
YELLOW "Check whether the above parameters are correct and start the installation in 3 seconds ..." && EXEC "sleep 3"

cd $HOME &> /dev/null && ls .cc-switch-server &> /dev/null && INFO "Backup old local data ..." && EXEC "mv -v .cc-switch-server .cc-switch-server.bak.$(date +s)"
EXEC "/usr/local/bin/${binary} init --router-url ${ROUTER} --owner-email ${OWNER} --password ${PASSWORD}"
EXEC "nohup /usr/local/bin/${binary} &> /dev/null &"
EXEC "sleep 3"
SUBDOMAIN=$(grep tunnelSubdomain $HOME/.cc-switch-server/server.json  | awk -F '"' '{print $(NF-1)}')
SUBDOMAIN_URL=$(echo ${ROUTER} | sed "s/:\/\//:\/\/${SUBDOMAIN}\./")

# info
YELLOW "Please visit ${SUBDOMAIN_URL} with the browser ..."
YELLOW "Web Password: ${PASSWORD}"

}

main $@
