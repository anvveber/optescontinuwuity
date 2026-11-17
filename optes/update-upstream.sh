#!/usr/bin/env bash
# update-upstream.sh — move this fork onto a newer continuwuity release.
#
# Usage (from the repo root, clean working tree):
#   optes/update-upstream.sh            # newest stable upstream tag
#   optes/update-upstream.sh v26.10.1   # a specific tag
#   optes/update-upstream.sh --test-only  # just build and run all tests
#
# What it does:
#   1. fetches upstream tags (GitHub mirror);
#   2. on a new branch update/<tag> from main, merges the new tag (a merge
#      commit, then a commit that records the tag in optes/UPSTREAM);
#   3. builds continuwuity and the tunnel, runs the unit and end-to-end tests.
# It never touches main. If everything passes, review and fast-forward main
# yourself (the script prints the command). On a merge conflict it stops and
# leaves the merge in progress for you to resolve.
#
# Why a merge and not a rebase: main is published. A rebase gives every fork
# commit a new hash, so main could only be pushed with --force and every
# commit cited in the docs would go stale. A merge keeps them.
#
# Commits are made in UTC and end with the same Co-Authored-By line as the
# rest of the history.

set -euo pipefail

UPSTREAM_URL=https://github.com/continuwuity/continuwuity
cd "$(git rev-parse --show-toplevel)"

# libclang for bindgen: the newest installed LLVM unless LIBCLANG_PATH is set
# (Ubuntu 24.04: llvm-18, Debian 13: llvm-19).
find_libclang() {
  local d
  for d in $(printf '%s\n' /usr/lib/llvm-*/lib | sort -t- -k2 -n -r); do
    compgen -G "$d/libclang*.so*" > /dev/null && { echo "$d"; return; }
  done
}

run_tests() {
  export LIBCLANG_PATH=${LIBCLANG_PATH:-$(find_libclang)}
  echo "==> build"
  cargo build -q -p conduwuit
  (cd optes/tunnel && cargo build -q && cargo test -q)
  # Test build: knows plain-TCP fronts, which normal builds refuse.
  (cd optes/tunnel && cargo build -q --features test-fronts --target-dir target/test-fronts)
  echo "==> unit tests (patches)"
  cargo test -q -p conduwuit_service --features url_preview --lib -- release:: sync::
  cargo test -q -p conduwuit_router -p conduwuit_service --features conduwuit_service/url_preview --lib -- guard_tests::
  echo "==> clock tool (decision rule)"
  optes/test/run-clock-decide-test.sh
  echo "==> end-to-end"
  optes/test/run-e2e.sh target/debug/conduwuit 15
  optes/test/run-e2e.sh target/debug/conduwuit 0
  optes/test/run-spaces-e2e.sh target/debug/conduwuit
  RELEASE=15 optes/test/run-spaces-e2e.sh target/debug/conduwuit
  optes/test/run-hardening-e2e.sh target/debug/conduwuit on
  optes/test/run-hardening-e2e.sh target/debug/conduwuit off
  optes/test/run-browser-guard-e2e.sh target/debug/conduwuit on
  optes/test/run-browser-guard-e2e.sh target/debug/conduwuit off
  optes/test/run-retention-e2e.sh target/debug/conduwuit
  RELEASE=15 optes/test/run-retention-e2e.sh target/debug/conduwuit
  for level in 1 2 3; do
    optes/test/run-tunnel-e2e.sh target/debug/conduwuit optes/tunnel/target/test-fronts/debug "$level"
  done
  optes/test/run-socks-path-e2e.sh target/debug/conduwuit optes/tunnel/target/debug tor
  optes/test/run-socks-path-e2e.sh target/debug/conduwuit optes/tunnel/target/debug i2p
}

if [[ ${1:-} == --test-only ]]; then
  run_tests
  echo "All tests passed."
  exit 0
fi

[[ -z $(git status --porcelain) ]] || { echo "working tree not clean" >&2; exit 1; }

old=$(cat optes/UPSTREAM)
git fetch -q --tags "$UPSTREAM_URL" 2>/dev/null || git fetch --tags "$UPSTREAM_URL"

new=${1:-$(git tag -l 'v[0-9]*' | grep -Ev -- '-(alpha|beta|rc)' | sort -V | tail -1)}
[[ -n $new ]] || { echo "no upstream tag found" >&2; exit 1; }
if [[ $new == "$old" ]]; then
  echo "already on $old"
  exit 0
fi
echo "==> $old -> $new"

trailer="Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
branch=update/$new
git checkout -q -B "$branch" main
if ! TZ=UTC git merge -q --no-ff -m "Merge upstream $new" -m "$trailer" "$new"; then
  echo
  echo "Merge conflict. Resolve and keep the behaviour of our patches, then:"
  echo "  TZ=UTC git commit        (the message is prepared; keep the trailer)"
  echo "  echo $new > optes/UPSTREAM"
  echo "  TZ=UTC git commit -am 'chore: track upstream $new' -m '$trailer'"
  echo "  optes/update-upstream.sh --test-only"
  exit 1
fi
echo "$new" > optes/UPSTREAM
TZ=UTC git commit -qam "chore: track upstream $new" -m "$trailer"

run_tests

echo
echo "All tests passed on $new (branch $branch)."
echo "Check upstream's changelog (CHANGELOG.md) for security fixes, new config"
echo "keys (add them to setup-core.sh and its CONFIG_ADDED) and database"
echo "migrations. Run cargo-audit on Cargo.lock. Then:"
echo "  git checkout main && git merge --ff-only $branch && git push"
