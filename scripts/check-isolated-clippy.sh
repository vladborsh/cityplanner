#!/usr/bin/env bash
# Runs clippy on each sim crate in its own cargo invocation, so Cargo resolves features for
# that crate alone. `cargo build/clippy --workspace` unifies features across the workspace
# and can hide a crate that does not build by itself (BUG-001: cp_core needed serde/std
# that only its neighbours enabled). Crate list: scripts/sim-crates.sh.
# Usage: scripts/check-isolated-clippy.sh
set -euo pipefail
cd "$(dirname "$0")/.."

# (Command substitution, not `< <(...)`, so a failing sim-crates.sh fails this script too.)
sim_list=$(scripts/sim-crates.sh)
CRATES=()
while IFS= read -r c; do CRATES+=("$c"); done <<<"$sim_list"
[ "${#CRATES[@]}" -gt 0 ] || { echo "no sim crates found" >&2; exit 2; }

failed=()
for crate in "${CRATES[@]}"; do
  echo "::group::clippy -p $crate"
  if ! cargo clippy -p "$crate" --all-targets --locked -- -D warnings; then
    failed+=("$crate")
  fi
  echo "::endgroup::"
done

if [ "${#failed[@]}" -ne 0 ]; then
  echo "FAIL: isolated clippy failed for: ${failed[*]}" >&2
  exit 1
fi
echo "OK: isolated clippy passed for ${#CRATES[@]} sim crates (${CRATES[*]})"
