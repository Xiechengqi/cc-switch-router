#!/usr/bin/env bash
set -Eeuo pipefail

readonly LITELLM_PRICE_URL="https://raw.githubusercontent.com/BerriAI/litellm/refs/heads/main/model_prices_and_context_window.json"
readonly LITELLM_SOURCE_URL="https://github.com/BerriAI/litellm/blob/main/model_prices_and_context_window.json"
readonly PRICE_CATALOG="pricing/model-prices.json"
readonly DERIVE_SCRIPT="scripts/pricing/derive-model-prices.mjs"
readonly AUDIT_SCRIPT="scripts/audit/audit-model-prices.mjs"

fail() {
  echo "git-push.sh: $*" >&2
  exit 1
}

for command_name in git curl node sha256sum; do
  command -v "$command_name" >/dev/null 2>&1 || fail "required command is unavailable: $command_name"
done

repository_root=$(git rev-parse --show-toplevel 2>/dev/null) || fail "not inside a Git repository"
cd "$repository_root"

[[ -f "$PRICE_CATALOG" ]] || fail "$PRICE_CATALOG is missing"
[[ -f "$DERIVE_SCRIPT" ]] || fail "$DERIVE_SCRIPT is missing"
[[ -f "$AUDIT_SCRIPT" ]] || fail "$AUDIT_SCRIPT is missing"

# Never absorb a developer's in-progress catalog edit into the automatic
# synchronization commit. Other dirty files are intentionally left untouched.
if ! git diff --quiet -- "$PRICE_CATALOG" || ! git diff --cached --quiet -- "$PRICE_CATALOG"; then
  fail "$PRICE_CATALOG has uncommitted changes; commit or restore them before pushing"
fi

temporary_directory=$(mktemp -d "${TMPDIR:-/tmp}/cc-switch-router-pricing.XXXXXX")
cleanup() {
  [[ -n "${temporary_directory:-}" && -d "$temporary_directory" ]] && rm -rf -- "$temporary_directory"
}
trap cleanup EXIT

downloaded_source="$temporary_directory/model_prices_and_context_window.json"
derived_catalog="$temporary_directory/model-prices.json"

echo "Checking the latest LiteLLM model prices..."
curl \
  --fail \
  --silent \
  --show-error \
  --location \
  --retry 3 \
  --retry-all-errors \
  --connect-timeout 15 \
  --max-time 120 \
  --output "$downloaded_source" \
  "$LITELLM_PRICE_URL"

node -e '
  const fs = require("node:fs");
  const value = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("LiteLLM price source must be a JSON object");
  }
' "$downloaded_source"

downloaded_sha=$(sha256sum "$downloaded_source" | awk '{print $1}')
catalog_sha=$(node -e '
  const fs = require("node:fs");
  const value = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  process.stdout.write(String(value?.source?.sha256 || ""));
' "$PRICE_CATALOG")

if [[ "$downloaded_sha" == "$catalog_sha" ]]; then
  echo "LiteLLM model prices are already current ($downloaded_sha)."
else
  echo "LiteLLM source changed: ${catalog_sha:-<missing>} -> $downloaded_sha"
  node "$DERIVE_SCRIPT" \
    --source "$downloaded_source" \
    --out "$derived_catalog" \
    --source-url "$LITELLM_SOURCE_URL"

  # Install only the generated repository-owned artifact. The upstream source
  # remains temporary and is never committed or used directly at runtime.
  install -m 0644 "$derived_catalog" "$PRICE_CATALOG"

  if ! node "$AUDIT_SCRIPT" --check; then
    git show "HEAD:$PRICE_CATALOG" > "$PRICE_CATALOG"
    fail "derived price catalog failed audit; restored the committed catalog"
  fi

  if git diff --quiet -- "$PRICE_CATALOG"; then
    echo "The derived catalog is unchanged; no pricing commit is needed."
  else
    git add -- "$PRICE_CATALOG"
    git commit -m "chore(pricing): sync LiteLLM model prices"
    echo "Created an independent LiteLLM pricing commit."
  fi
fi

echo "Pushing Git commits..."
git push "$@"
