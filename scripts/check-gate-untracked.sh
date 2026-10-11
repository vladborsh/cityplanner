#!/usr/bin/env bash
# Regression check for BUG-013: a branch whose commit references a file that was never
# `git add`ed must be refused by gate.sh and ff-merge.sh, and master must not move.
# Works in a scratch clone (never touches the real coordinator checkout); no cargo needed,
# because both refusals happen before any build step.
set -euo pipefail
cd "$(dirname "$0")/.."
src=$PWD
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

git clone -q "$src" "$tmp/coord"
git -C "$tmp/coord" checkout -q -B master "$(git rev-parse HEAD)"
# Use the scripts under test (including uncommitted edits) inside the clone.
cp scripts/gate.sh scripts/ff-merge.sh "$tmp/coord/scripts/"
git -C "$tmp/coord" -c user.name=t -c user.email=t@t commit -qam "scratch: scripts under test" || true
git -C "$tmp/coord" worktree add -q -b probe "$tmp/wt" master
cd "$tmp/wt"
echo 'pub mod zzprobe;' >>crates/cp_core/src/lib.rs
git -c user.name=t -c user.email=t@t commit -qam "probe: references an untracked file"
echo 'pub fn probe() {}' >crates/cp_core/src/zzprobe.rs

before=$(git -C "$tmp/coord" rev-parse HEAD)
rc=0
CITYPLANNER_COORD="$tmp/coord" scripts/ff-merge.sh probe >"$tmp/ff.log" 2>&1 || rc=$?
[ "$rc" -ne 0 ] || { echo "FAIL: ff-merge accepted a branch with an untracked file"; exit 1; }
rc=0
scripts/gate.sh >"$tmp/gate.log" 2>&1 || rc=$?
[ "$rc" -ne 0 ] || { echo "FAIL: gate.sh accepted an untracked file"; exit 1; }
grep -q '^FAIL: untracked$' "$tmp/gate.log" || { echo "FAIL: gate.sh failed for another reason"; exit 1; }
[ "$(git -C "$tmp/coord" rev-parse HEAD)" = "$before" ] || { echo "FAIL: master moved"; exit 1; }
echo "PASS: untracked files are refused, master unchanged"
