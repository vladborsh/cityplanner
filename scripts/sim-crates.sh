#!/usr/bin/env bash
# Prints the sim crates, one per line, sorted: cp_sim, every workspace crate it depends on
# (transitively), and cp_save (which sits above cp_sim, so it is not in cp_sim's tree).
# The single source of this list for scripts/check-sim-deps.sh and the isolated-clippy CI job.
# Exit 2 if the list cannot be derived.
set -euo pipefail
cd "$(dirname "$0")/.."

EXTRA_SIM_CRATES=(cp_save)

# Workspace members show up in `cargo tree` as "name vX.Y.Z (/path)"; registry crates have no path.
sim=$(cargo tree --target all --edges normal,build,dev --prefix none --format '{p}' -p cp_sim \
  | awk '$3 ~ /^\(\// {print $1}' | sort -u)
case $'\n'"$sim"$'\n' in *$'\ncp_sim\n'*) ;; *) echo "sim-crates error: cp_sim not found in its own tree" >&2; exit 2;; esac

members=$(cargo metadata --no-deps --format-version 1)
for crate in "${EXTRA_SIM_CRATES[@]}"; do
  grep -q "\"name\":\"$crate\"" <<<"$members" \
    || { echo "sim-crates error: $crate is not a workspace member" >&2; exit 2; }
done

printf '%s\n' "$sim" "${EXTRA_SIM_CRATES[@]}" | sort -u
