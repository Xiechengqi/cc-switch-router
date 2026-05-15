#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

if [[ -f frontend/package.json ]]; then
  (cd frontend && npm ci && npm run build)
fi

cargo build --release

cp -f -v target/release/cc-switch-router .
