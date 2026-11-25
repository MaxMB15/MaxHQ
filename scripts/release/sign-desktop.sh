#!/bin/sh
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# Signs each Windows and Linux installer in a folder for the app's updater,
# as package-macos.sh signs the Mac's updater archive: <asset>.sig, with the
# version in its trusted comment, and in the bridge release of a key change
# also <asset>.next.sig with the next key. latest-json.mjs --desktop-sigs
# reads them. It needs Node and nothing else.
#
#   sh scripts/release/sign-desktop.sh <dir>
#
# The installers are named as bundles.mjs has them. One that isn't in the
# folder, because its build failed, is left unsigned with a warning, and its
# platform keeps the version it has. A folder with none fails.
#
# The keys and the key change come from the environment, as package-macos.sh
# reads them: TAURI_SIGNING_PRIVATE_KEY and the rest (see sign.mjs),
# MAXHQ_UPDATE_KEY=next to sign with the next key, and MAXHQ_BRIDGE=1 to
# sign with the current key and the next one too.

set -eu

RELEASE=$(cd "$(dirname "$0")" && pwd)
. "$RELEASE/lib.sh"

[ $# -eq 1 ] || die "usage: sign-desktop.sh <dir>"
dir=$1
[ -d "$dir" ] || die "$dir isn't a folder"

choose_update_key
check_update_keys "the update key doesn't read"

version=$(node "$RELEASE/version.mjs" get)
assets=$(node "$RELEASE/bundles.mjs" assets)
signed=0
for asset in $assets; do
	file=$dir/$asset
	rm -f "$file.sig" "$file.next.sig"
	if [ ! -f "$file" ]; then
		warn "$asset isn't in $dir, so it has no signature and its platform keeps the version it has."
		continue
	fi
	# shellcheck disable=SC2086 # $key_flag is empty or one flag
	node "$RELEASE/sign.mjs" $key_flag --app-version "$version" "$file"
	if [ "$bridge" = 1 ]; then
		node "$RELEASE/sign.mjs" --next --app-version "$version" --out "$file.next.sig" "$file"
	fi
	signed=$((signed + 1))
done
[ "$signed" -gt 0 ] || die "$dir has none of the installers: $(echo "$assets" | tr '\n' ' ')"
echo "Signed $signed installers for $version."
