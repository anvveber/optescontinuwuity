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
#   2. rebases our commits from the current base (optes/UPSTREAM) onto the new
#      tag, on a new branch update/<tag>;
#   3. builds continuwuity and the tunnel, runs the unit and end-to-end tests.
# It never touches main. If everything passes, review and fast-forward main
# yourself (the script prints the command). On a rebase conflict it stops and
# leaves the rebase in progress for you to resolve.

set -euo pipefail

UPSTREAM_URL=https://github.com/continuwuity/continuwuity
cd "$(git rev-parse --show-toplevel)"

run_tests() {
  export LIBCLANG_PATH=${LIBCLANG_PATH:-/usr/lib/llvm-18/lib}
  echo "==> build"
  cargo build -q -p conduwuit
  (cd optes/tunnel && cargo build -q && cargo test -q)
  echo "==> unit tests (patches)"
  cargo test -q -p conduwuit_service --features url_preview --lib -- release:: sync::
  echo "==> end-to-end"
  optes/test/run-e2e.sh target/debug/conduwuit 15
  optes/test/run-e2e.sh target/debug/conduwuit 0
  optes/test/run-spaces-e2e.sh target/debug/conduwuit
  RELEASE=15 optes/test/run-spaces-e2e.sh target/debug/conduwuit
  optes/test/run-hardening-e2e.sh target/debug/conduwuit on
  optes/test/run-hardening-e2e.sh target/debug/conduwuit off
  for level in 1 2 3; do
    optes/test/run-tunnel-e2e.sh target/debug/conduwuit optes/tunnel/target/debug "$level"
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

branch=update/$new
git checkout -q -B "$branch" main
if ! git rebase --onto "$new" "$old" "$branch"; then
  echo
  echo "Rebase conflict. Resolve, 'git rebase --continue', then rerun the tests:"
  echo "  optes/update-upstream.sh --test-only"
  exit 1
fi
echo "$new" > optes/UPSTREAM
git commit -qam "chore: track upstream $new"

run_tests

echo
echo "All tests passed on $new (branch $branch)."
echo "Check upstream's changelog for config changes, then:"
echo "  git checkout main && git merge --ff-only $branch && git push"
