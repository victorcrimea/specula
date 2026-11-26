#!/bin/sh
# Applies engine/patches to a clean 0 A.D. v0.28.0 checkout.
#
# usage: scripts/apply-patches.sh DIR [--am]
# With --am each patch becomes a commit, keeping its message, so the result
# can be exported again; by default the changes are left in the work tree.

set -eu

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
	echo "usage: $0 DIR [--am]" >&2
	exit 2
fi

root=$(cd "$(dirname "$0")/.." && pwd)
dir=$1
mode=${2:-}

if [ "$(git -C "$dir" rev-parse HEAD)" != "$(git -C "$dir" rev-parse 'v0.28.0^{commit}')" ]; then
	echo "$dir is not at v0.28.0" >&2
	exit 1
fi
if [ -n "$(git -C "$dir" status --porcelain)" ]; then
	echo "$dir is not clean" >&2
	exit 1
fi

for p in "$root"/engine/patches/*.patch; do
	if [ "$mode" = "--am" ]; then
		git -C "$dir" am --quiet "$p"
	else
		git -C "$dir" apply "$p"
	fi
done
