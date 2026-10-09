#!/usr/bin/env bash
# Dependency guard (architecture/system-design.md §1, AGENTS.md "Simulation crates are headless").
# Every crate up to and including cp_sim (and cp_save) must not pull in rendering, windowing,
# UI or profiling (cp_prof) crates.
# Sim crates = cp_sim, every workspace crate it depends on (transitively), and cp_save (above cp_sim).
# Usage: scripts/check-sim-deps.sh [extra-banned-crate ...]
set -euo pipefail
cd "$(dirname "$0")/.."

# cp_prof (phase timing, uses Instant) is a non-sim crate: sim crates must never depend on it.
# Banning a name that is not in the workspace (yet) is harmless: matching is by exact name.
BANNED=(bevy bevy_render bevy_pbr bevy_winit bevy_egui winit cp_prof "$@")

# Package names in `cargo tree` output look like "name vX.Y.Z (/path)" for workspace members.
tree() { cargo tree --target all --edges normal,build,dev --prefix none --format '{p}' "$@"; }

# Sim crate list is shared with the isolated-clippy CI job: scripts/sim-crates.sh.
# (Command substitution, not `< <(...)`, so a failing sim-crates.sh fails this script too.)
sim_list=$(scripts/sim-crates.sh)
SIM_CRATES=()
while IFS= read -r c; do SIM_CRATES+=("$c"); done <<<"$sim_list"

fail=0
for crate in "${SIM_CRATES[@]}"; do
  deps=$(tree -p "$crate" | awk '{print $1}' | sort -u)
  for banned in "${BANNED[@]}"; do
    if grep -qx "$banned" <<<"$deps"; then
      echo "FAIL: $crate depends on banned crate '$banned' (see: cargo tree -p $crate -i $banned)" >&2
      fail=1
    fi
  done
done

if [ "$fail" -ne 0 ]; then
  echo "Sim crates must stay headless: no rendering/windowing/UI dependencies." >&2
  exit 1
fi
echo "OK: ${#SIM_CRATES[@]} sim crates are headless (${SIM_CRATES[*]})"
