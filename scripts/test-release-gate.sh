#!/usr/bin/env bash
# Scratch-history tests for scripts/release-gate.sh: `npm run test:release-gate`.
# Each case builds a repository whose last feature merge was 100 hours ago,
# adds one commit made now, and says whether the gate must stop a tag.
set -uo pipefail

gate="$(cd "$(dirname "$0")" && pwd)/release-gate.sh"
root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT
failures=0

# new_repo <name>: a repository with a tagged release, 100 hours old.
new_repo() {
  repo="$root/$1"
  mkdir -p "$repo" && cd "$repo" || exit 1
  git init -q
  git config user.email t@example.com
  git config user.name t
  git config core.autocrlf false
  mkdir -p src .github/workflows .github/ISSUE_TEMPLATE
  printf 'version = "0.1.0"\nrussh = "0.5"\n' >Cargo.toml
  printf 'name = "a"\nversion = "1.0.0"\nchecksum = "aa"\n' >Cargo.lock
  echo one >src/main.rs
  echo "version: 0.1.0" >.github/ISSUE_TEMPLATE/bug_report.yml
  echo "on: push" >.github/workflows/ci.yml
  git add -A
  old=$(($(date +%s) - 100 * 3600))
  GIT_AUTHOR_DATE="$old +0000" GIT_COMMITTER_DATE="$old +0000" git commit -qm "feature"
  git tag v0.1.0
}

commit_now() {
  git add -A
  git commit -qm "$1"
}

# expect <fail|pass> <name>: the gate on HEAD, as a tag.
expect() {
  "$gate" HEAD tag >/dev/null 2>&1
  code=$?
  got=pass
  [ "$code" -eq 0 ] || got=fail
  if [ "$got" = "$1" ]; then
    echo "ok   $2"
  else
    echo "FAIL $2 (exit $code, wanted $1)"
    failures=$((failures + 1))
  fi
}

new_repo version-bump # the release's own bump, with the issue template (B2)
sed -i 's/0.1.0/0.1.1/' Cargo.toml Cargo.lock .github/ISSUE_TEMPLATE/bug_report.yml
echo "## 0.1.1" >CHANGELOG.md
commit_now bump
expect pass "a version bump with the issue template restarts nothing"

new_repo dependency-bump
sed -i 's/0.5/0.6/' Cargo.toml
sed -i 's/aa/bb/' Cargo.lock
commit_now bump
expect fail "a dependency bump restarts the wait"

new_repo workflow-change
echo "on: [push, pull_request]" >.github/workflows/ci.yml
commit_now ci
expect fail "a workflow change restarts the wait"

new_repo docs-only
mkdir -p docs && echo x >docs/a.md
commit_now docs
expect pass "documentation restarts nothing"

new_repo source-change
echo two >src/main.rs
commit_now code
expect fail "a source change restarts the wait"

new_repo large-lock-diff # a diff past the pipe buffer must not read as version-only (B3)
{
  for i in $(seq 1 9000); do printf '[[package]]\nname = "p%s"\nversion = "1.0.%s"\nchecksum = "%064d"\n' "$i" "$i" "$i"; done
} >Cargo.lock
git add -A && old=$(($(date +%s) - 100 * 3600)) && GIT_COMMITTER_DATE="$old +0000" git commit -qm "locks"
git tag v0.1.1
sed -i 's/checksum = "0*\([0-9]\)/checksum = "x\1/' Cargo.lock
commit_now "cargo update"
expect fail "a very large lock-file bump restarts the wait"

new_repo bad-commit
if "$gate" nosuchcommit tag >/dev/null 2>&1; then
  echo "FAIL an unknown commit must stop the gate"
  failures=$((failures + 1))
else
  echo "ok   an unknown commit stops the gate"
fi

[ "$failures" -eq 0 ] || { echo "$failures failed"; exit 1; }
