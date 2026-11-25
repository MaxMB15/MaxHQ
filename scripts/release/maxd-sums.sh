#!/bin/sh
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# Writes SHA256SUMS for the maxd binaries a release carries as assets, in
# the folder that holds them, and prints the assets to upload, one per line,
# SHA256SUMS last. The app checks a maxd it downloads against its line.
#
#   sh scripts/release/maxd-sums.sh <dir>
#
# Both Linux maxd must be there. Each Windows maxd (bundles.mjs maxd names
# them) is added when it's there, and left out with a warning when its build
# failed, so the Linux and Mac release goes out without it.

set -eu

RELEASE=$(cd "$(dirname "$0")" && pwd)
. "$RELEASE/lib.sh"

[ $# -eq 1 ] || die "usage: maxd-sums.sh <dir>"
dir=$1
[ -d "$dir" ] || die "$dir isn't a folder"

names=""
for name in maxd-x86_64-unknown-linux-musl maxd-aarch64-unknown-linux-musl; do
	[ -f "$dir/$name" ] || die "$dir has no $name"
	names="$names $name"
done
for name in $(node "$RELEASE/bundles.mjs" maxd); do
	if [ -f "$dir/$name" ]; then
		names="$names $name"
	else
		warn "$dir has no $name, so the release goes out without it."
	fi
done

if command -v sha256sum > /dev/null 2>&1; then
	sum() { sha256sum "$@"; }
else
	sum() { shasum -a 256 "$@"; }
fi
# shellcheck disable=SC2086 # $names is a list of plain file names
(cd "$dir" && sum $names > SHA256SUMS)
cat "$dir/SHA256SUMS" >&2
for name in $names SHA256SUMS; do echo "$name"; done
