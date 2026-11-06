#!/usr/bin/env bash
# Keep a renamed Rust filter from silently passing after matching zero tests.
set -euo pipefail

if (( $# == 0 )); then
  echo 'usage: require-tests.sh COMMAND [ARGUMENTS...]' >&2
  exit 2
fi

test_log=$(mktemp)
trap 'rm -f "$test_log"' EXIT
"$@" | tee "$test_log"
passed_count=$(awk '/^test result: ok\./ { total += $4 } END { print total + 0 }' "$test_log")
if (( passed_count == 0 )); then
  echo 'Rust filter did not execute any passing tests; refusing an empty gate.' >&2
  exit 1
fi
printf 'Rust filter executed %s passing tests.\n' "$passed_count"
if [[ -n ${GITHUB_STEP_SUMMARY:-} ]]; then
  printf -- '- Rust filter (%s): %s passed\n' "$*" "$passed_count" >> "$GITHUB_STEP_SUMMARY"
fi
