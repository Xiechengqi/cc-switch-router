#!/usr/bin/env bash
set -euo pipefail

: "${GITHUB_REPOSITORY:?missing repository}"
: "${GITHUB_SHA:?missing commit}"
: "${ROUTER_RELEASE_VERSION:?missing version}"
[[ $GITHUB_REPOSITORY =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]]
[[ $GITHUB_SHA =~ ^[0-9a-f]{40}$ ]]
test -s release-files/cc-switch-router-linux-amd64

# Publication has its own non-cancelling concurrency group. Check freshness
# inside that group, not before waiting for it. API/network failures fail closed.
current_sha=$(git ls-remote --exit-code origin refs/heads/master | cut -f1)
if [[ $current_sha != "$GITHUB_SHA" ]]; then
  echo "Skipping superseded commit $GITHUB_SHA; master is $current_sha."
  exit 0
fi

# Create the tag only if missing. Do not move an existing tag or update public
# release metadata until the new asset upload has succeeded.
if ! gh api "repos/$GITHUB_REPOSITORY/git/ref/tags/latest" --silent; then
  gh api --method POST "repos/$GITHUB_REPOSITORY/git/refs" \
    -f ref=refs/tags/latest -f "sha=$GITHUB_SHA" --silent
fi

notes=$(printf '## Latest automated build\n\nVersion: v%s\nCommit: %s\nBranch: master\n\n### Assets\n\n- Ubuntu AMD64 binary: `cc-switch-router-linux-amd64`\n\n### Quick download\n\n```bash\nwget https://github.com/%s/releases/download/latest/cc-switch-router-linux-amd64 -O /usr/local/bin/cc-switch-router && chmod +x /usr/local/bin/cc-switch-router\n```\n' "$ROUTER_RELEASE_VERSION" "$GITHUB_SHA" "$GITHUB_REPOSITORY")
title="Latest Build (v$ROUTER_RELEASE_VERSION)"
if ! gh release view latest --repo "$GITHUB_REPOSITORY" >/dev/null; then
  gh release create latest --repo "$GITHUB_REPOSITORY" --verify-tag --draft --title "$title" --notes "$notes"
fi
# GitHub's --clobber replaces a same-named asset non-atomically. Keep the release
# itself in place; a failed upload must fail CI and must not claim new metadata.
gh release upload latest release-files/cc-switch-router-linux-amd64 --repo "$GITHUB_REPOSITORY" --clobber
gh api --method PATCH "repos/$GITHUB_REPOSITORY/git/refs/tags/latest" \
  -f "sha=$GITHUB_SHA" -F force=true --silent
gh release edit latest --repo "$GITHUB_REPOSITORY" --draft=false --title "$title" --notes "$notes" --latest
