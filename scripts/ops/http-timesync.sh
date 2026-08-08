#!/usr/bin/env bash

set -uo pipefail

readonly LOCK_FILE="/run/cc-switch-http-timesync.lock"
readonly PROBE_TIMEOUT_SECS=4
readonly SOURCE_AGREEMENT_MS=2500
readonly CORRECTION_THRESHOLD_MS=2000
readonly DRY_RUN="${CC_SWITCH_HTTP_TIMESYNC_DRY_RUN:-0}"
readonly SOURCES=(
  "https://www.cloudflare.com/cdn-cgi/trace"
  "https://www.apple.com/library/test/success.html"
  "https://checkip.amazonaws.com/"
)

log() {
  printf '%s cc-switch-http-timesync: %s\n' "$(date --utc '+%Y-%m-%dT%H:%M:%SZ')" "$*"
}

alert() {
  log "$*"
  if command -v logger >/dev/null 2>&1; then
    logger --priority daemon.err --tag cc-switch-http-timesync -- "$*"
  fi
}

for command_name in curl date flock mktemp sort awk timeout wc tr rm; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    alert "required command is unavailable: $command_name"
    exit 1
  fi
done

exec 9>"$LOCK_FILE"
if ! flock --nonblock 9; then
  log "another correction check is already running"
  exit 0
fi

task_tmp_dir="$(mktemp -d /tmp/cc-switch-http-timesync.XXXXXX)" || exit 1
trap 'rm -rf -- "$task_tmp_dir"' EXIT

probe_source() {
  local index="$1"
  local url="$2"
  local headers_file="$task_tmp_dir/headers-$index"
  local result_file="$task_tmp_dir/result-$index"
  local log_file="$task_tmp_dir/log-$index"
  local started_ns finished_ns date_header age_header remote_secs midpoint_ms reference_ms http_code

  started_ns="$(date +%s%N)" || return
  if ! http_code="$(curl \
    --silent \
    --show-error \
    --fail \
    --connect-timeout 2 \
    --max-time "$PROBE_TIMEOUT_SECS" \
    --max-redirs 0 \
    --proto '=https' \
    --tlsv1.2 \
    --header 'Cache-Control: no-cache' \
    --dump-header "$headers_file" \
    --output /dev/null \
    --write-out '%{http_code}' \
    "$url" 2>"$log_file")"; then
    printf 'source=%s status=failed error=%s\n' "$url" "$(tr '\n' ' ' <"$log_file")" >"$log_file"
    return
  fi
  if [[ ! "$http_code" =~ ^2[0-9][0-9]$ ]]; then
    printf 'source=%s status=failed error=HTTP-%s\n' "$url" "$http_code" >"$log_file"
    return
  fi
  finished_ns="$(date +%s%N)" || return
  date_header="$(awk '{ line=$0; if (tolower(substr(line, 1, 5)) == "date:") { sub(/^[^:]*:[[:space:]]*/, "", line); sub(/\r$/, "", line); value=line } } END { print value }' "$headers_file")"
  age_header="$(awk '{ line=$0; if (tolower(substr(line, 1, 4)) == "age:") { sub(/^[^:]*:[[:space:]]*/, "", line); sub(/\r$/, "", line); value=line } } END { print value }' "$headers_file")"
  if [[ -z "$date_header" ]]; then
    printf 'source=%s status=failed error=missing-Date-header\n' "$url" >"$log_file"
    return
  fi
  if [[ "$age_header" =~ ^[0-9]+$ ]] && (( age_header > 5 )); then
    printf 'source=%s status=failed error=cached-response age_seconds=%s\n' "$url" "$age_header" >"$log_file"
    return
  fi
  if ! remote_secs="$(LC_ALL=C date --utc --date "$date_header" +%s 2>/dev/null)"; then
    printf 'source=%s status=failed error=invalid-Date-header\n' "$url" >"$log_file"
    return
  fi

  midpoint_ms=$(( (started_ns + finished_ns) / 2000000 ))
  reference_ms=$(( remote_secs * 1000 + 500 ))
  printf '%s\n' "$(( midpoint_ms - reference_ms ))" >"$result_file"
  printf 'source=%s status=ok offset_ms=%s rtt_ms=%s\n' \
    "$url" \
    "$(( midpoint_ms - reference_ms ))" \
    "$(( (finished_ns - started_ns) / 1000000 ))" >"$log_file"
}

for index in "${!SOURCES[@]}"; do
  probe_source "$index" "${SOURCES[$index]}" &
done
wait

for index in "${!SOURCES[@]}"; do
  if [[ -f "$task_tmp_dir/log-$index" ]]; then
    while IFS= read -r line; do
      log "$line"
    done <"$task_tmp_dir/log-$index"
  fi
done

offsets_file="$task_tmp_dir/offsets"
for index in "${!SOURCES[@]}"; do
  if [[ -f "$task_tmp_dir/result-$index" ]]; then
    read -r offset <"$task_tmp_dir/result-$index"
    if [[ "$offset" =~ ^-?[0-9]+$ ]]; then
      printf '%s\n' "$offset"
    fi
  fi
done | sort --numeric-sort >"$offsets_file"

if [[ "$(wc -l <"$offsets_file")" -lt 2 ]]; then
  alert "external time quorum unavailable"
  exit 0
fi

consensus="$(awk -v limit="$SOURCE_AGREEMENT_MS" '
  { value[++count] = $1 }
  END {
    best_start = 0
    best_count = 0
    for (start = 1; start <= count; start++) {
      for (finish = start; finish <= count; finish++) {
        if (value[finish] - value[start] > limit) break
        size = finish - start + 1
        if (size > best_count) {
          best_start = start
          best_count = size
        }
      }
    }
    if (best_count < 2) exit 2
    middle = best_start + int(best_count / 2)
    if (best_count % 2 == 1) {
      median = value[middle]
    } else {
      median = int((value[middle - 1] + value[middle]) / 2)
    }
    printf "%d %d\n", median, best_count
  }
' "$offsets_file")"
if [[ -z "$consensus" ]]; then
  alert "external time sources disagree; correction skipped"
  exit 0
fi
read -r offset_ms quorum_sources <<<"$consensus"
log "quorum_sources=$quorum_sources local_minus_reference_ms=$offset_ms"

absolute_offset_ms="${offset_ms#-}"
if (( absolute_offset_ms <= CORRECTION_THRESHOLD_MS )); then
  log "clock is within the correction threshold"
  exit 0
fi

ntp_synchronized="unknown"
if command -v timedatectl >/dev/null 2>&1; then
  ntp_value="$(timeout 2s timedatectl show --property=NTPSynchronized --value 2>/dev/null || true)"
  case "${ntp_value,,}" in
    yes|true|1) ntp_synchronized="yes" ;;
    no|false|0) ntp_synchronized="no" ;;
  esac
fi

if [[ "$ntp_synchronized" == "yes" ]]; then
  alert "NTP reports synchronized but HTTPS quorum disagrees by ${offset_ms}ms; correction skipped"
  exit 0
fi
if [[ "$ntp_synchronized" != "no" ]]; then
  alert "NTP synchronization state is unknown and clock differs by ${offset_ms}ms; correction skipped"
  exit 0
fi

if [[ "$DRY_RUN" == "1" ]]; then
  log "dry run: would correct the clock by $((-offset_ms))ms"
  exit 0
fi

current_ms=$(( $(date +%s%N) / 1000000 ))
target_ms=$(( current_ms - offset_ms ))
target_seconds=$(( target_ms / 1000 ))
target_millis=$(( target_ms % 1000 ))
printf -v target_timestamp '@%d.%03d' "$target_seconds" "$target_millis"
if ! date --utc --set "$target_timestamp" >/dev/null; then
  alert "clock correction failed for offset ${offset_ms}ms"
  exit 1
fi
log "clock corrected by $((-offset_ms))ms using HTTPS quorum"

in_container=false
if command -v systemd-detect-virt >/dev/null 2>&1 \
  && systemd-detect-virt --quiet --container; then
  in_container=true
fi
if [[ "$in_container" == "false" ]] \
  && { [[ -e /dev/rtc ]] || [[ -e /dev/rtc0 ]]; } \
  && command -v hwclock >/dev/null 2>&1; then
  if timeout 5s hwclock --systohc --utc; then
    log "hardware clock updated"
  else
    alert "system clock was corrected but writing the hardware clock failed"
  fi
fi
