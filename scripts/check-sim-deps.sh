#!/usr/bin/env bash
# Dependency guard (architecture/system-design.md §1, AGENTS.md "Simulation crates are headless").
# Every crate up to and including cp_sim must not pull in rendering, windowing or UI crates.
# Sim crates = cp_sim plus every workspace crate that cp_sim depends on (transitively).
# Usage: scripts/check-sim-deps.sh [extra-banned-crate ...]
set -euo pipefail
cd "$(dirname "$0")/.."

BANNED=(bevy bevy_render bevy_pbr bevy_winit bevy_egui winit "$@")

# Package names in `cargo tree` output look like "name vX.Y.Z (/path)" for workspace members.
tree() { cargo tree --target all --edges normal,build,dev --prefix none --format '{p}' "$@"; }

# Workspace members that cp_sim reaches (path deps carry a "(/...)" suffix).
SIM_CRATES=()
while IFS= read -r c; do SIM_CRATES+=("$c"); done < <(tree -p cp_sim | awk '$3 ~ /^\(\// {print $1}' | sort -u)
case " ${SIM_CRATES[*]} " in *" cp_sim "*) ;; *) echo "guard error: cp_sim not found in its own tree" >&2; exit 2;; esac

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
