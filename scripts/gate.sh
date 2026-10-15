#!/usr/bin/env bash
# The local merge gate: the definition of "green" (AGENTS.md "Build and test").
#
# Runs, in order, each as its own command whose exit status is checked:
#   fmt, clippy, test, doctest, sim-deps, isolated-clippy
# and stops at the first failure. Prints exactly one final line, `PASS: N tests` (exit 0) or
# `FAIL: <step>` (exit 1). N is the tests nextest ran and passed (`cargo nextest run`) plus the
# doctests that passed (`cargo test --doc`). cargo-nextest is required (no silent fallback to
# `cargo test`): install it with `cargo install cargo-nextest --locked`. Test threads are capped
# (GATE_TEST_THREADS, default 6) so a gate fits in memory next to another slot's build.
#
# Why it is written this way: cargo output is redirected to a log file, never piped, so a
# pipeline such as `cargo test | tail` cannot hide a non-zero exit status. Do not add a `|`
# after a cargo command here. Logs: $CARGO_TARGET_DIR/gate/<step>.log (target/ if unset).
#
# It also refuses (FAIL: untracked) when the tree has untracked, non-ignored files: a file the
# branch needs but forgot to `git add` would pass here and break master after the merge. Add it
# or gitignore it. (Run in a clean checkout instead? Rejected: a new path rebuilds everything.)
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

if ! cargo nextest --version >/dev/null 2>&1; then
  echo "-- cargo-nextest is not installed; install it with: cargo install cargo-nextest --locked"
  fail "nextest-missing"
fi

untracked=$(git ls-files --others --exclude-standard)
if [ -n "$untracked" ]; then
  echo "-- untracked files (git add them, or gitignore them); the gate must test what is committed:"
  echo "$untracked"
  fail "untracked"
fi

run_step fmt cargo fmt --check
run_step clippy cargo clippy --workspace --all-targets -- -D warnings
run_step test cargo nextest run --workspace --test-threads "${GATE_TEST_THREADS:-6}" --status-level fail --final-status-level fail
run_step doctest cargo test --workspace --doc
run_step sim-deps scripts/check-sim-deps.sh
run_step isolated-clippy scripts/check-isolated-clippy.sh

# nextest's summary line: "     Summary [  12.3s] 727 tests run: 727 passed (3 slow), 0 skipped".
nextest_tests=$(awk '
  /^ *Summary \[/ { for (i = 2; i <= NF; i++) if ($i == "passed" || $i == "passed,") total += $(i - 1) }
  END { print total + 0 }
' "$LOG_DIR/test.log")
# Doctests: "test result: ok. 2 passed; 0 failed; ..." per crate.
doc_tests=$(awk '
  /^test result:/ { for (i = 2; i <= NF; i++) if ($i == "passed;") total += $(i - 1) }
  END { print total + 0 }
' "$LOG_DIR/doctest.log")
tests=$((nextest_tests + doc_tests))
if [ "$nextest_tests" -le 0 ]; then
  fail "test (no tests were counted in $LOG_DIR/test.log)"
fi
echo "PASS: $tests tests"
