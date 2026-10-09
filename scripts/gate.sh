#!/usr/bin/env bash
# The local merge gate: the definition of "green" (AGENTS.md "Build and test").
#
# Runs, in order, each as its own command whose exit status is checked:
#   fmt, clippy, test, sim-deps, isolated-clippy
# and stops at the first failure. Prints exactly one final line, `PASS: N tests` (exit 0) or
# `FAIL: <step>` (exit 1). N is the sum over every `test result:` line of `cargo test`.
#
# Why it is written this way: cargo output is redirected to a log file, never piped, so a
# pipeline such as `cargo test | tail` cannot hide a non-zero exit status. Do not add a `|`
# after a cargo command here. Logs: $CARGO_TARGET_DIR/gate/<step>.log (target/ if unset).
#
# Portable bash (macOS 3.2 and Linux 5). Uses the caller's CARGO_TARGET_DIR; hardcodes none.
set -euo pipefail
cd "$(dirname "$0")/.."

LOG_DIR="${CARGO_TARGET_DIR:-$PWD/target}/gate"
mkdir -p "$LOG_DIR"
export CARGO_TERM_COLOR=never

fail() {
  echo "FAIL: $1"
  exit 1
}

# run_step <name> <command...>: output to $LOG_DIR/<name>.log; on failure show the log tail.
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

run_step fmt cargo fmt --check
run_step clippy cargo clippy --workspace --all-targets -- -D warnings
run_step test cargo test --workspace
run_step sim-deps scripts/check-sim-deps.sh
run_step isolated-clippy scripts/check-isolated-clippy.sh

# Sum "N passed" over every libtest summary line ("test result: ok. 12 passed; 0 failed; ...").
tests=$(awk '
  /^test result:/ { for (i = 2; i <= NF; i++) if ($i == "passed;") total += $(i - 1) }
  END { print total + 0 }
' "$LOG_DIR/test.log")
if [ "$tests" -le 0 ]; then
  fail "test (no tests were counted in $LOG_DIR/test.log)"
fi
echo "PASS: $tests tests"
