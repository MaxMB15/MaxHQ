#!/bin/sh
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# Checks a packaged MaxHQ.app on the Mac that built it. It fails when:
#
# - a file the release layout names is missing from the bundle
# - a Mac binary in the app has no code for this Mac's CPU
# - a Mac binary links a library from Homebrew, MacPorts or /usr/local by its
#   full path, which a user's Mac won't have
# - Contents/MacOS/maxd doesn't print its version
# - a Linux maxd isn't listed in SHA256SUMS with its own sum, or isn't in
#   maxd-release.json with it
# - with --require-updates, the app has no update endpoint
# - the Mac maxd's sum isn't the one maxd-release.json lists
#
# It doesn't start the app.
#
#   sh scripts/release/smoke-test.sh path/to/MaxHQ.app [--require-linux] [--require-sig] [--require-updates]

set -eu

app=${1:?usage: smoke-test.sh <MaxHQ.app> [--require-linux] [--require-sig] [--require-updates]}
shift
require_linux=0
require_sig=0
require_updates=0
for flag in "$@"; do
	case "$flag" in
	--require-linux) require_linux=1 ;;
	--require-sig) require_sig=1 ;;
	--require-updates) require_updates=1 ;;
	*)
		echo "smoke-test.sh: unknown argument $flag" >&2
		exit 2
		;;
	esac
done

failed=0
fail() {
	echo "Error: $*" >&2
	failed=1
}

cpu=$(uname -m)
case "$cpu" in
arm64) mac_target=aarch64-apple-darwin ;;
x86_64) mac_target=x86_64-apple-darwin ;;
*) mac_target=unknown ;;
esac
macos=$app/Contents/MacOS
resources=$app/Contents/Resources
manifest=$resources/maxd-release.json

for f in "$macos/maxhq-desktop" "$macos/maxd" "$macos/maxd-vm" "$manifest"; do
	[ -f "$f" ] || fail "${f#"$app"/} is missing"
done
if [ "$require_sig" = 1 ] && [ ! -s "$manifest.sig" ]; then
	fail "Contents/Resources/maxd-release.json.sig is missing"
fi
# The updater's endpoint goes in at build time (MAXHQ_RELEASES_REPO), and
# without it the app never looks for an update.
if [ "$require_updates" = 1 ] && ! grep -aq '/releases/latest/download/latest.json' "$macos/maxhq-desktop"; then
	fail "the app has no update endpoint; build it with MAXHQ_RELEASES_REPO set"
fi

# Each Mach-O binary in the bundle, one path per line. The bundle's paths
# have no newlines.
machos=$(find "$app/Contents" -type f -exec sh -c 'file -b "$1" | grep -q "^Mach-O"' sh {} \; -print)
[ -n "$machos" ] || fail "found no Mac binaries in $app"
echo "Checking $(printf '%s\n' "$machos" | grep -c .) Mac binaries for $cpu"
outside='^(/opt/homebrew|/opt/local|/usr/local)/'
while IFS= read -r f; do
	[ -n "$f" ] || continue
	rel=${f#"$app"/}
	archs=$(lipo -archs "$f")
	case " $archs " in
	*" $cpu "*) ;;
	*) fail "$rel is built for $archs, not $cpu" ;;
	esac
	for lib in $(otool -L "$f" | awk 'NR > 1 { print $1 }' | grep -E "$outside" || true); do
		fail "$rel links $lib"
	done
done <<EOF
$machos
EOF

if [ -x "$macos/maxd" ]; then
	if version=$(env -u DYLD_LIBRARY_PATH -u DYLD_FALLBACK_LIBRARY_PATH "$macos/maxd" --version); then
		echo "maxd --version: $version"
	else
		fail "Contents/MacOS/maxd --version failed"
	fi
fi

# The sum a manifest or SHA256SUMS lists for a name.
listed() {
	node -e 'const m = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
		process.stdout.write(m.binaries[process.argv[2]] ?? "")' "$manifest" "$1"
}
sum_of() { shasum -a 256 "$1" | cut -d' ' -f1; }

if [ -f "$manifest" ]; then
	[ "$(listed "maxd-$mac_target")" = "$(sum_of "$macos/maxd")" ] ||
		fail "maxd-release.json doesn't list Contents/MacOS/maxd's sum under maxd-$mac_target"
fi
for cpu_name in x86_64 aarch64; do
	name=maxd-$cpu_name-unknown-linux-musl
	f=$resources/$name
	if [ ! -f "$f" ]; then
		[ "$require_linux" = 1 ] && fail "Contents/Resources/$name is missing"
		continue
	fi
	sum=$(sum_of "$f")
	grep -qx "$sum  $name" "$resources/SHA256SUMS" 2>/dev/null ||
		fail "SHA256SUMS doesn't list $name with its sum"
	[ "$(listed "$name")" = "$sum" ] || fail "maxd-release.json doesn't list $name with its sum"
done

if [ "$failed" = 1 ]; then
	exit 1
fi
echo "$(basename "$app") has the release layout, and its binaries run on $cpu."
