#!/usr/bin/env bash
# The per-merge quick gate (user decision 2026-10-17): the full gate (scripts/gate.sh) runs on
# master in batches (scripts/full-gate-master.sh, by the Tech Lead); every merge runs this.
#
# Usage: scripts/quick-gate.sh <base-sha>   (ff-merge passes the coordinator's master)
#
# Steps, each checked: untracked files, fmt, then clippy -D warnings and nextest on the CHANGED
# crates (crates whose directory has files changed in <base>...HEAD) and their direct workspace
# dependents. Changes under
# assets/content or fixtures add cp_content, cp_sim and cp_save (content hash, pins, saves).
# A change to the root Cargo.toml / Cargo.lock / clippy.toml / rust-toolchain falls back to
# the full gate. Docs/tasks-only changes run fmt only. A Cargo.toml change also runs the
# sim-deps check. Prints `PASS: N tests (quick: <crates>)` or `FAIL: <step>`.
#
# Cargo output goes to log files, never piped (see gate.sh). Portable bash (macOS 3.2).
set -euo pipefail
cd "$(dirname "$0")/.."

[ "$#" -eq 1 ] || { echo "usage: scripts/quick-gate.sh <base-sha>"; exit 2; }
base=$1

LOG_DIR="${CARGO_TARGET_DIR:-$PWD/target}/gate"
mkdir -p "$LOG_DIR"
export CARGO_TERM_COLOR=never

fail() {
  echo "FAIL: $1"
  exit 1
}

run_step() {
  local name=$1
  shift
  local log="$LOG_DIR/$name.log"
  echo "== $name: $*"
  local rc=0
  "$@" >"$log" 2>&1 || rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "-- $name failed (exit $rc); last lines of $log:"
    tail -n 40 "$log"
    fail "$name"
  fi
}

untracked=$(git ls-files --others --exclude-standard)
if [ -n "$untracked" ]; then
  echo "-- untracked files (git add them, or gitignore them):"
  echo "$untracked"
  fail "untracked"
fi

changed=$(git diff --name-only "$base"...HEAD)

# Workspace-wide build settings changed: the quick gate cannot judge it.
if echo "$changed" | grep -qE '^(Cargo\.toml|Cargo\.lock|clippy\.toml|rust-toolchain(\.toml)?|\.cargo/)'; then
  echo "-- root build settings changed: running the full gate"
  exec scripts/gate.sh
fi

run_step fmt cargo fmt --check

# crates/<name>/..., apps/<name>/..., tools/<name>/... -> the package name in that Cargo.toml.
crates=""
for dir in $(echo "$changed" | awk -F/ '($1=="crates"||$1=="apps"||$1=="tools") && NF>2 {print $1"/"$2}' | sort -u); do
  [ -f "$dir/Cargo.toml" ] || continue
  name=$(awk -F'"' '/^name *=/ {print $2; exit}' "$dir/Cargo.toml")
  [ -n "$name" ] && crates="$crates $name"
done
crates=$(echo "$crates" | tr ' ' '\n' | awk 'NF' | sort -u | tr '\n' ' ')

# Direct reverse dependencies inside the workspace too (a cp_net change once broke a cp_render
# test that no quick gate ran: BUG-073). Only one level: deeper dependents wait for the batch gate.
if [ -n "$crates" ]; then
  rdeps=$(cargo metadata --no-deps --format-version 1 2>/dev/null | python3 -c '
import json, sys
changed = set(sys.argv[1:])
meta = json.load(sys.stdin)
for p in meta["packages"]:
    if any(d["name"] in changed and d.get("path") for d in p["dependencies"]):
        print(p["name"])' $crates)
  crates=$(printf '%s\n%s\n' "$crates" "$rdeps" | tr ' ' '\n' | awk 'NF' | sort -u | tr '\n' ' ')
fi
# Content/fixture changes add the crates that read them (not their dependents: content is data).
if echo "$changed" | grep -qE '^(assets/content/|fixtures/)'; then
  crates=$(printf '%s\ncp_content\ncp_sim\ncp_save\n' "$crates" | tr ' ' '\n' | awk 'NF' | sort -u | tr '\n' ' ')
fi

if [ -z "$crates" ]; then
  echo "PASS: 0 tests (quick: no crate changed)"
  exit 0
fi

pkgs=""
for c in $crates; do pkgs="$pkgs -p $c"; done

# shellcheck disable=SC2086
run_step clippy cargo clippy $pkgs --all-targets -- -D warnings
# shellcheck disable=SC2086
run_step test cargo nextest run $pkgs --test-threads "${GATE_TEST_THREADS:-6}" --status-level fail --final-status-level fail

if echo "$changed" | grep -qE '(^|/)Cargo\.toml$'; then
  run_step sim-deps scripts/check-sim-deps.sh
fi

tests=$(awk '
  /^ *Summary \[/ { for (i = 2; i <= NF; i++) if ($i == "passed" || $i == "passed,") total += $(i - 1) }
  END { print total + 0 }' "$LOG_DIR/test.log")
echo "PASS: $tests tests (quick: $crates)"
