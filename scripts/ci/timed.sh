#!/usr/bin/env bash
set -uo pipefail

if (( $# < 2 )); then
  echo 'usage: timed.sh LABEL COMMAND [ARGUMENTS...]' >&2
  exit 2
fi
label=$1
shift
started=$SECONDS
"$@"
status=$?
elapsed=$((SECONDS - started))
printf '%s: %ss (exit %s)\n' "$label" "$elapsed" "$status"
if [[ -n ${GITHUB_STEP_SUMMARY:-} ]]; then
  printf -- '- %s: %ss (exit %s)\n' "$label" "$elapsed" "$status" >> "$GITHUB_STEP_SUMMARY"
fi
exit "$status"
