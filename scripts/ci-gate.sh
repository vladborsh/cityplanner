#!/usr/bin/env bash
# The GitHub CI gate (user decision 2026-10-27): only the required checks, and no test that
# needs a GPU or a window. The merge gate stays the local scripts/gate.sh.
#
#   scripts/ci-gate.sh
#
# Steps, each its own command, stopping at the first failure: fmt, clippy -D warnings on the
# whole workspace (builds every target, runs nothing), the sim-deps and isolated-clippy checks,
# then nextest and doctests of the sim crates only (scripts/sim-crates.sh; the determinism test
# is in cp_sim). Tests of cp_render, cp_ui, cp_tools, cp_app and apps/* do not run here.
# Output in $CARGO_TARGET_DIR/gate/ci-<step>.log; prints `PASS: N tests` or `FAIL: <step>`.
# Portable bash (macOS 3.2 and Linux 5).
set -euo pipefail
cd "$(dirname "$0")/.."

LOG_DIR="${CARGO_TARGET_DIR:-$PWD/target}/gate"
mkdir -p "$LOG_DIR"
export CARGO_TERM_COLOR=never

fail() {
  echo "FAIL: $1"
  exit 1
}

# run_step <name> <command...>: output to $LOG_DIR/ci-<name>.log; on failure show the log tail.
run_step() {
  local name=$1
  shift
  local log="$LOG_DIR/ci-$name.log"
  echo "== $name: $*"
  local rc=0
  "$@" >"$log" 2>&1 || rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "-- $name failed (exit $rc); last lines of $log:"
    tail -n 60 "$log"
    fail "$name"
  fi
}

if ! cargo nextest --version >/dev/null 2>&1; then
  echo "-- cargo-nextest is not installed; install it with: cargo install cargo-nextest --locked"
  fail "nextest-missing"
fi

sim_crates=$(scripts/sim-crates.sh) || fail "sim-crates"
packages=()
for c in $sim_crates; do
  packages+=(-p "$c")
done

run_step fmt cargo fmt --all --check
run_step clippy cargo clippy --locked --workspace --all-targets -- -D warnings
run_step sim-deps scripts/check-sim-deps.sh
run_step isolated-clippy scripts/check-isolated-clippy.sh
run_step test cargo nextest run --locked "${packages[@]}" --test-threads "${GATE_TEST_THREADS:-4}" --status-level fail --final-status-level fail
run_step doctest cargo test --locked "${packages[@]}" --doc

nextest_tests=$(awk '
  /^ *Summary \[/ { for (i = 2; i <= NF; i++) if ($i == "passed" || $i == "passed,") total += $(i - 1) }
  END { print total + 0 }
' "$LOG_DIR/ci-test.log")
doc_tests=$(awk '
  /^test result:/ { for (i = 2; i <= NF; i++) if ($i == "passed;") total += $(i - 1) }
  END { print total + 0 }
' "$LOG_DIR/ci-doctest.log")
if [ "$nextest_tests" -le 0 ]; then
  fail "test (no tests were counted in $LOG_DIR/ci-test.log)"
fi
echo "PASS: $((nextest_tests + doc_tests)) tests (sim crates)"
