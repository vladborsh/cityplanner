#!/usr/bin/env bash
# Prints the sim crates, one per line, sorted. The single source of this list for
# scripts/check-sim-deps.sh and scripts/check-isolated-clippy.sh.
#
# A sim crate is mechanically a workspace member whose Cargo.toml has `[lints] workspace = true`
# (the workspace lints ban HashMap/HashSet etc.). Non-sim crates opt out explicitly with their
# own `[lints.clippy]`. So a new sim crate is covered the moment it adopts the workspace lints,
# even if cp_sim does not depend on it yet.
#
# Cross-check (architecture/system-design.md §1): every workspace crate in cp_sim's dependency
# tree, plus cp_save (which sits above cp_sim), must be in the manifest list. Otherwise a crate
# that sim code depends on has quietly opted out of the sim rules: exit 2 naming it.
# Exit 2 also if the list cannot be derived.
set -euo pipefail
cd "$(dirname "$0")/.."

REQUIRED_EXTRA=(cp_save)

die() { echo "sim-crates error: $*" >&2; exit 2; }

# --- 1. From manifests: workspace members with `[lints]` / `workspace = true` --------------
# Members as "name<TAB>manifest_path" (cargo metadata is JSON; python3 is on every CI runner).
members=$(cargo metadata --no-deps --format-version 1 | python3 -I -c '
import json, sys
for p in json.load(sys.stdin)["packages"]:
    print(p["name"] + "\t" + p["manifest_path"])')
[ -n "$members" ] || die "no workspace members found"

# True if the manifest has a top-level `[lints]` table containing `workspace = true`.
inherits_workspace_lints() {
  awk '
    /^[[:space:]]*\[/ { in_lints = ($0 ~ /^[[:space:]]*\[lints\][[:space:]]*(#.*)?$/); next }
    in_lints && /^[[:space:]]*workspace[[:space:]]*=[[:space:]]*true[[:space:]]*(#.*)?$/ { found = 1 }
    END { exit found ? 0 : 1 }
  ' "$1"
}

MANIFEST_LIST=()
while IFS=$'\t' read -r name path; do
  if inherits_workspace_lints "$path"; then MANIFEST_LIST+=("$name"); fi
done <<<"$members"
[ "${#MANIFEST_LIST[@]}" -gt 0 ] || die "no workspace member has [lints] workspace = true"
SORTED_MANIFEST=$(printf '%s\n' "${MANIFEST_LIST[@]}" | sort -u)

# --- 2. Cross-check against cp_sim's tree + cp_save ----------------------------------------
# Workspace members show up in `cargo tree` as "name vX.Y.Z (/path)"; registry crates have no path.
tree=$(cargo tree --target all --edges normal,build,dev --prefix none --format '{p}' -p cp_sim \
  | awk '$3 ~ /^\(\// {print $1}' | sort -u)
grep -qx cp_sim <<<"$tree" || die "cp_sim not found in its own tree"

for crate in "${REQUIRED_EXTRA[@]}"; do
  cut -f1 <<<"$members" | grep -qx "$crate" || die "$crate is not a workspace member"
done

missing=$( { printf '%s\n' "$tree" "${REQUIRED_EXTRA[@]}"; } | sort -u | comm -23 - <(printf '%s\n' "$SORTED_MANIFEST") )
if [ -n "$missing" ]; then
  while IFS= read -r crate; do
    echo "sim-crates error: $crate is in cp_sim's dependency tree (or is cp_save) but its Cargo.toml lacks '[lints] workspace = true'; sim crates must not opt out of the workspace lints" >&2
  done <<<"$missing"
  exit 2
fi

printf '%s\n' "$SORTED_MANIFEST"
