#!/bin/sh
# Regenerates engine/patches from the commits the engine worktree has on top
# of 0 A.D. v0.28.0, one patch per commit.
#
# usage: scripts/export-patches.sh
# ENGINE_DIR overrides the worktree (default ../0ad-specula).

set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
engine=${ENGINE_DIR:-$root/../0ad-specula}
out=$root/engine/patches

if [ -n "$(git -C "$engine" status --porcelain)" ]; then
	echo "$engine has uncommitted changes; commit them first" >&2
	exit 1
fi

# Stale numbers would otherwise survive when commits are squashed or dropped.
rm -f "$out"/*.patch
mkdir -p "$out"
git -C "$engine" format-patch --quiet --no-signature -o "$out" v0.28.0..HEAD
ls "$out"/*.patch | wc -l
