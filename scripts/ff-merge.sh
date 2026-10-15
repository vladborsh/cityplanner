#!/usr/bin/env bash
# Gated fast-forward merge into the coordinator checkout's master.
# Usage (from the worktree where <branch> is checked out): scripts/ff-merge.sh <branch>
#
# Refuses (exit 1) unless ALL of these hold, checked in this order, merging only on PASS:
#   1. the coordinator checkout ($CITYPLANNER_COORD) is on master with a clean tree
#      (tracked files; this runs before the gate, so a refusal costs nothing);
#   2. <branch> is checked out in THIS worktree, with a clean tree (untracked, non-ignored files count as dirty), so the gate tests exactly
#      the commit that will land;
#   3. <branch> contains the coordinator's current master ("rebase first" otherwise);
#   4. scripts/gate.sh prints PASS (fmt, clippy, nextest, doctests, sim checks; needs cargo-nextest);
#   5. master did not move while the gate ran.
# Then: git -C $CITYPLANNER_COORD merge --ff-only <branch>. Never pushes.
#
# CITYPLANNER_COORD defaults to /Users/Vladyslav_Borsh/Documents/cityplanner.
# Portable bash (macOS 3.2 and Linux 5).
set -euo pipefail
cd "$(dirname "$0")/.."

COORD="${CITYPLANNER_COORD:-/Users/Vladyslav_Borsh/Documents/cityplanner}"

die() {
  echo "ff-merge: $*" >&2
  exit 1
}

[ "$#" -eq 1 ] || die "usage: scripts/ff-merge.sh <branch>"
branch=$1

git -C "$COORD" rev-parse --git-dir >/dev/null 2>&1 || die "$COORD is not a git checkout (set CITYPLANNER_COORD)"
[ "$(git -C "$COORD" symbolic-ref --short HEAD 2>/dev/null || true)" = "master" ] \
  || die "coordinator checkout $COORD is not on master"
coord_dirty=$(git -C "$COORD" status --porcelain --untracked-files=no)
[ -z "$coord_dirty" ] || die "coordinator checkout $COORD has uncommitted changes; refusing before the gate"

git rev-parse --verify --quiet "refs/heads/$branch" >/dev/null || die "no local branch '$branch'"
[ "$(git symbolic-ref --short HEAD 2>/dev/null || true)" = "$branch" ] \
  || die "check out '$branch' in this worktree first (the gate tests this worktree)"
here_dirty=$(git status --porcelain)
[ -z "$here_dirty" ] || die "this worktree has uncommitted or untracked files (git add/commit or gitignore them) so the gate tests what lands:
$here_dirty"

master_sha=$(git -C "$COORD" rev-parse HEAD)
git merge-base --is-ancestor "$master_sha" "$branch" \
  || die "'$branch' does not contain the coordinator's master ($master_sha): rebase first"

echo "ff-merge: running the gate on $branch ($(git rev-parse --short HEAD))"
gate_rc=0
scripts/gate.sh || gate_rc=$?
[ "$gate_rc" -eq 0 ] || die "gate failed (exit $gate_rc); nothing merged"

now_sha=$(git -C "$COORD" rev-parse HEAD)
[ "$now_sha" = "$master_sha" ] || die "master moved during the gate ($master_sha -> $now_sha): rebase and rerun"

merge_rc=0
git -C "$COORD" merge --ff-only "$branch" >/dev/null || merge_rc=$?
[ "$merge_rc" -eq 0 ] || die "git merge --ff-only failed (exit $merge_rc)"
echo "MERGED: $branch -> master $(git -C "$COORD" rev-parse --short HEAD) (not pushed)"
