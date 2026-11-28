#!/usr/bin/env bash
# The part of the release gate (docs/dev/releasing.md, "The release gate")
# that a workflow can check. Run by .github/workflows/release.yml before
# anything is built; run it by hand in a clone to see where a tag would stand:
#
#   scripts/release-gate.sh <commit> [tag|dry-run]
#
# Checks, for the commit to be released:
#   1. CI (ci.yml) finished green on exactly that commit (needs GH_TOKEN and
#      GITHUB_REPOSITORY; skipped with a note when they are not set).
#   2. 48 hours have passed since the last feature merge: the newest commit
#      since the previous v* tag that changes anything but documentation,
#      the changelog and the version lines of the manifests. A dependency
#      bump, a lock file change or a change to a workflow or action is a feature
#      merge; issue templates and the like are documentation.
# With "dry-run" (a build by hand, which never publishes) the failures are
# printed as warnings and the script exits 0.
#
# What it cannot check, and the maintainer does: that the smoke test and the
# lab suite ran on this commit and their output is attached to the release's
# ticket, and that no tag is cut in the middle of a wave.
set -euo pipefail

commit=${1:?usage: release-gate.sh <commit> [tag|dry-run]}
mode=${2:-tag}
wait_hours=${GATE_WAIT_HOURS:-48}
failed=0

# A gate never passes by accident: needs a real commit and the full history.
given=$commit
commit=$(git rev-parse --verify "$given^{commit}") || { echo "release gate: no such commit: $given" >&2; exit 2; }
if [ "$(git rev-parse --is-shallow-repository)" = true ]; then
  echo "release gate: this clone is shallow; fetch the full history and the tags first." >&2
  exit 2
fi

fail() {
  if [ "$mode" = dry-run ]; then
    echo "WARNING (would stop a tag): $*"
  else
    echo "::error::release gate: $*"
    failed=1
  fi
}

# 1. CI on this commit.
if [ -n "${GH_TOKEN:-}" ] && [ -n "${GITHUB_REPOSITORY:-}" ]; then
  conclusions=$(gh run list --repo "$GITHUB_REPOSITORY" --workflow ci.yml \
    --commit "$commit" --json conclusion --jq '.[].conclusion' || true)
  if ! printf '%s\n' "$conclusions" | grep -qx success; then
    fail "CI has no green run on $commit. Wait for it, or re-run it, before tagging."
  elif printf '%s\n' "$conclusions" | grep -qx -e failure -e cancelled -e timed_out; then
    echo "note: CI also has a failed run on $commit; a green run exists, so the gate passes."
  else
    echo "CI is green on $commit."
  fi
else
  echo "note: no GH_TOKEN or GITHUB_REPOSITORY; CI status not checked."
fi

# 2. 48 hours since the last feature merge.
previous=$(git describe --tags --abbrev=0 --match 'v*' "$commit^" 2>/dev/null || true)
range=${previous:+$previous..}$commit
last=""

# Manifests and lock files: a change there is no feature merge only when
# every changed line is a version line (the release's own version bump).
# A dependency bump changes other lines (checksums, integrity, ranges).
manifests=(Cargo.toml Cargo.lock package.json package-lock.json src-tauri/tauri.conf.json 'crates/*/Cargo.toml')
version_only() {
  local diff
  # No -q below and no hidden errors: a grep that quits early would kill the
  # pipe upstream (SIGPIPE) and read as "version only" on a large lock diff.
  diff=$(git diff -U0 "$1^" "$1" -- "${manifests[@]}") || return 1
  ! printf '%s
' "$diff" | grep -E '^[-+]' | grep -vE '^(\+\+\+|---) ' |
    grep -vE '^[-+][[:space:]]*"?version"?[[:space:]]*[:=]' >/dev/null
}

while read -r sha stamp; do
  files=$(git diff-tree --root --no-commit-id --name-only -r -m --first-parent "$sha")
  # Everything outside documentation, the changelog and the manifests.
  if printf '%s\n' "$files" | grep -vE '^(docs/|\.github/(ISSUE_TEMPLATE/|pull_request_template\.md$|CODEOWNERS$)|CHANGELOG\.md$|README\.md$|ROADMAP\.md$|CLAUDE\.md$|Cargo\.toml$|Cargo\.lock$|package\.json$|package-lock\.json$|src-tauri/tauri\.conf\.json$|crates/[^/]+/Cargo\.toml$|$)' | grep . >/dev/null; then
    last="$sha $stamp"
    break
  fi
  # Only manifests, lock files and docs: a feature merge unless the manifest
  # changes are version lines alone.
  if printf '%s\n' "$files" | grep -qE '^(Cargo\.toml|Cargo\.lock|package\.json|package-lock\.json|src-tauri/tauri\.conf\.json|crates/[^/]+/Cargo\.toml)$'; then
    if [ "$(git rev-list --parents -n1 "$sha" | wc -w)" -lt 2 ] || ! version_only "$sha"; then
      last="$sha $stamp"
      break
    fi
  fi
done < <(git log --first-parent --format='%H %ct' "$range")

if [ -z "$last" ]; then
  echo "No feature merge since ${previous:-the start of history}; nothing to wait for."
else
  sha=${last% *}
  stamp=${last#* }
  now=$(date +%s)
  age=$(((now - stamp) / 3600))
  echo "Last feature merge since ${previous:-the start}: $(git log -1 --format='%h %s' "$sha"), $age hours ago."
  if [ $((now - stamp)) -lt $((wait_hours * 3600)) ]; then
    fail "only $age hours since the last feature merge ($(git log -1 --format='%h' "$sha")); $wait_hours must pass before the tag."
  fi
fi

exit $failed
