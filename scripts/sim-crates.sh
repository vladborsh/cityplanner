#!/usr/bin/env bash
# Prints the sim crates, one per line, sorted. The single source of this list for
# scripts/check-sim-deps.sh and scripts/check-isolated-clippy.sh.
#
# A sim crate is mechanically a workspace member whose Cargo.toml has `lints.workspace = true`
# (any TOML spelling: `[lints]` table, dotted key or inline table), which applies the workspace
# lints (ban HashMap/HashSet etc.). Non-sim crates opt out explicitly with their
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

# --- 1. From manifests: workspace members that inherit the workspace lints ------------------
# Read with a real TOML parser (python3 tomllib, stdlib since 3.11) so every spelling Cargo
# accepts counts: a `[lints]` table with `workspace = true`, the dotted key `lints.workspace =
# true`, or `lints = { workspace = true }`. A manifest that cannot be parsed is an error, never
# a silent omission. Output: member names on stdout, all member names on fd 3.
members_and_sim=$(cargo metadata --no-deps --format-version 1 | python3 -I -c '
import json, sys
try:
    import tomllib
except ImportError:
    sys.exit("python3 >= 3.11 (tomllib) is required to read workspace manifests")
for pkg in json.load(sys.stdin)["packages"]:
    try:
        with open(pkg["manifest_path"], "rb") as f:
            manifest = tomllib.load(f)
    except (OSError, tomllib.TOMLDecodeError) as e:
        sys.exit("cannot read manifest %s: %s" % (pkg["manifest_path"], e))
    lints = manifest.get("lints")
    inherits = isinstance(lints, dict) and lints.get("workspace") is True
    print(("sim\t" if inherits else "member\t") + pkg["name"])
') || die "could not read workspace manifests (see above)"
members=$(awk -F'\t' '{print $2}' <<<"$members_and_sim")
[ -n "$members" ] || die "no workspace members found"
SORTED_MANIFEST=$(awk -F'\t' '$1 == "sim" {print $2}' <<<"$members_and_sim" | sort -u)
[ -n "$SORTED_MANIFEST" ] || die "no workspace member inherits the workspace lints (lints.workspace = true)"

# --- 2. Cross-check against cp_sim's tree + cp_save ----------------------------------------
# Workspace members show up in `cargo tree` as "name vX.Y.Z (/path)"; registry crates have no path.
tree=$(cargo tree --target all --edges normal,build,dev --prefix none --format '{p}' -p cp_sim \
  | awk '$3 ~ /^\(\// {print $1}' | sort -u)
grep -qx cp_sim <<<"$tree" || die "cp_sim not found in its own tree"

for crate in "${REQUIRED_EXTRA[@]}"; do
  grep -qx "$crate" <<<"$members" || die "$crate is not a workspace member"
done

missing=$( { printf '%s\n' "$tree" "${REQUIRED_EXTRA[@]}"; } | sort -u | comm -23 - <(printf '%s\n' "$SORTED_MANIFEST") )
if [ -n "$missing" ]; then
  while IFS= read -r crate; do
    echo "sim-crates error: $crate is in cp_sim's dependency tree (or is cp_save) but its Cargo.toml lacks '[lints] workspace = true'; sim crates must not opt out of the workspace lints" >&2
  done <<<"$missing"
  exit 2
fi

printf '%s\n' "$SORTED_MANIFEST"
