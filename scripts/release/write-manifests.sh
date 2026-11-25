#!/bin/sh
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# Writes the update manifests the app's updater reads, as the release's
# publish-latest-json job uploads them and the release dry run checks them,
# each checked against the key its readers trust. It prints the files to
# upload on stdout, one per line, and the manifests themselves on stderr.
#
#   sh scripts/release/write-manifests.sh <notes> <mac sigs> <desktop sigs> <out dir>
#
# <notes> is the release notes file. <mac sigs> holds each Mac's
# MaxHQ_<version>_<arch>.app.tar.gz.sig, and its .next.sig in the bridge
# release, as package-macos.sh writes them. <desktop sigs> holds the
# installers' signatures, as sign-desktop.sh writes them. The manifests go
# into <out dir>.
#
# The environment has the release and prepare-release's key change:
# VERSION, GITHUB_REPOSITORY, KEY_STATE (old, bridge or new), SIGN_KEY
# (current or next), NEXT_PUBKEY, UPDATE_MANIFEST (the manifest this
# release's apps read), CURRENT_PUBKEY (tauri.conf.json's key when it isn't
# set) and, with KEY_STATE=new, FROZEN: the previous release's latest.json.
#
# Without a key change, only latest.json, as always. The bridge release
# writes latest.json for apps on the current key and latest-v2.json for the
# next. After it, latest-v2.json is the live one and latest.json stays at
# the bridge release, so an app that skipped the bridge still reaches it
# first. The Windows and Linux installers go only into UPDATE_MANIFEST, so
# latest.json stays the Mac's.

set -eu

RELEASE=$(cd "$(dirname "$0")" && pwd)
. "$RELEASE/lib.sh"

[ $# -eq 4 ] || die "usage: write-manifests.sh <notes> <mac sigs> <desktop sigs> <out dir>"
notes=$1 mac=$2 desktop=$3 out=$4
: "${VERSION:?}" "${GITHUB_REPOSITORY:?}" "${UPDATE_MANIFEST:?}"
current=${CURRENT_PUBKEY:-$(node -e 'console.log(require(process.argv[1]).plugins.updater.pubkey)' \
	"$RELEASE/../../apps/desktop/src-tauri/tauri.conf.json")}
signer=$current
if [ "${SIGN_KEY:-}" = next ]; then signer=${NEXT_PUBKEY:?}; fi

# manifest <name> <signature suffix> <public key>
manifest() {
	name=$1 suffix=$2 key=$3
	set -- --sig "aarch64=$mac/MaxHQ_${VERSION}_aarch64.app.tar.gz$suffix" \
		--sig "x64=$mac/MaxHQ_${VERSION}_x64.app.tar.gz$suffix"
	if [ "$name" = "$UPDATE_MANIFEST" ]; then
		set -- "$@" --desktop-sigs "$desktop" --sig-suffix "$suffix"
	fi
	node "$RELEASE/latest-json.mjs" --version "$VERSION" --repo "$GITHUB_REPOSITORY" \
		--notes-file "$notes" --out "$out/$name" --key "$key" "$@" >&2
	cat "$out/$name" >&2
	echo "$out/$name"
}

mkdir -p "$out"
case "${KEY_STATE:-}" in
old)
	manifest latest.json .sig "$signer"
	;;
bridge)
	manifest latest.json .sig "$current"
	manifest latest-v2.json .next.sig "${NEXT_PUBKEY:?}"
	;;
new)
	manifest latest-v2.json .sig "$signer"
	[ -f "${FROZEN:-}" ] || die "KEY_STATE=new needs FROZEN, the previous release's latest.json"
	frozen=$(node -p 'require(require("node:path").resolve(process.argv[1])).version' "$FROZEN")
	[ "$frozen" != "$VERSION" ] || die "latest.json on ${PREVIOUS:-the previous release} already names $VERSION."
	cp "$FROZEN" "$out/latest.json"
	echo "latest.json stays as ${PREVIOUS:-the previous release} has it." >&2
	echo "$out/latest.json"
	;;
*)
	die "prepare-release gave no update key state."
	;;
esac
