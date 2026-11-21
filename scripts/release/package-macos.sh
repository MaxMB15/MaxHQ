#!/bin/sh
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# Builds MaxHQ.app for this Mac's CPU with everything a release carries in it,
# then the DMG and the updater archive. The release workflow runs `build` in
# a job with no secret and `assemble` in a separate signing job, which runs
# nothing the build made. A local build can run both.
#
#   sh scripts/release/package-macos.sh [build|assemble|all] [options]
#
# build      cargo build of maxd and maxd-vm, then `tauri build --bundles app`.
#            It never sees a signing key or Apple secret.
# assemble   puts the binaries in the app and signs everything, in this order:
#            codesign maxd and maxd-vm, add the Linux maxd and SHA256SUMS to
#            Resources, write and sign maxd-release.json, codesign the app,
#            notarize and staple it, make the DMG (signed, notarized and
#            stapled), then the updater archive and its .sig.
# all        both (the default).
#
# Options:
#   --linux-dir DIR   folder holding maxd-x86_64-unknown-linux-musl and
#                     maxd-aarch64-unknown-linux-musl. A missing one is skipped
#                     with a warning.
#   --require-linux   fail instead when either Linux binary is missing.
#   --notarize        notarize and staple the app and the DMG. Needs
#                     APPLE_API_KEY, APPLE_API_ISSUER and APPLE_API_KEY_PATH.
#   --out DIR         where the DMG and the updater archive go. Default
#                     target/release/bundle/release.
#
# Environment:
#   MAXHQ_SIGN_IDENTITY, or else APPLE_SIGNING_IDENTITY: the codesign
#     identity. Without either, everything is signed ad hoc (`-`).
#   TAURI_SIGNING_PRIVATE_KEY, or TAURI_SIGNING_PRIVATE_KEY_PATH, and
#     TAURI_SIGNING_PRIVATE_KEY_PASSWORD: the update key that signs
#     maxd-release.json and the updater archive (see sign.mjs). The same
#     names ending in _NEXT hold the next key during a key change.
#   MAXHQ_UPDATE_KEY=next: sign with the next key instead of the current
#     one. MAXHQ_BRIDGE=1: sign with the current key, and also sign the
#     updater archive with the next key, into <archive>.next.sig. A key
#     change's steps are in .github/RELEASING.md.
#   MAXHQ_RELEASES_REPO: the GitHub repository, as owner/name, whose
#     releases the app updates from. The build writes its
#     releases/latest/download/<manifest> into the updater's endpoints.
#     Without it the app can't look for updates.
#   MAXHQ_UPDATE_MANIFEST: that manifest's name. Default latest.json.
#   MAXHQ_UPDATE_PUBKEY: the public key the app's updater trusts, when it
#     isn't the one in tauri.conf.json.
#   MAXHQ_UNSIGNED_UPDATES=1: skip both update-key signatures, for a local
#     build without the key. Update everything then asks before it sends
#     this bundle's maxd to other devices.
#   CARGO_TARGET_DIR: as cargo reads it. Default target/.

set -eu

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
RELEASE=$ROOT/scripts/release
. "$RELEASE/retry.sh"

phase=all
case "${1:-}" in
build | assemble | all)
	phase=$1
	shift
	;;
esac
linux_dir=
require_linux=0
notarize=0
target_dir=${CARGO_TARGET_DIR:-$ROOT/target}
out=$target_dir/release/bundle/release
while [ $# -gt 0 ]; do
	case "$1" in
	--linux-dir)
		linux_dir=${2:?--linux-dir needs a folder}
		shift 2
		;;
	--require-linux)
		require_linux=1
		shift
		;;
	--notarize)
		notarize=1
		shift
		;;
	--out)
		out=${2:?--out needs a folder}
		shift 2
		;;
	*)
		echo "package-macos.sh: unknown argument $1" >&2
		exit 2
		;;
	esac
done

note() { printf '==> %s\n' "$*"; }
warn() {
	if [ -n "${GITHUB_ACTIONS:-}" ]; then
		echo "::warning::$*"
	else
		echo "warning: $*" >&2
	fi
}
die() {
	if [ -n "${GITHUB_ACTIONS:-}" ]; then
		echo "::error::$*"
	else
		echo "package-macos.sh: $*" >&2
	fi
	exit 1
}

[ "$(uname -s)" = Darwin ] || die "this builds the Mac app, so it runs on macOS only"
case "$(uname -m)" in
arm64)
	arch=aarch64
	mac_target=aarch64-apple-darwin
	;;
x86_64)
	arch=x64
	mac_target=x86_64-apple-darwin
	;;
*) die "unknown CPU $(uname -m)" ;;
esac

app=$target_dir/release/bundle/macos/MaxHQ.app

build() {
	note "Building maxd, maxd-vm and MaxHQ.app"
	# No signing key or Apple secret reaches the build, which runs every
	# package's scripts. Tauri would sign and notarize with the Apple ones,
	# before the binaries below are in the bundle.
	set -- env -u APPLE_SIGNING_IDENTITY -u APPLE_CERTIFICATE -u APPLE_CERTIFICATE_PASSWORD \
		-u APPLE_API_KEY -u APPLE_API_ISSUER -u APPLE_API_KEY_PATH -u APPLE_ID -u APPLE_PASSWORD \
		-u TAURI_SIGNING_PRIVATE_KEY -u TAURI_SIGNING_PRIVATE_KEY_PATH \
		-u TAURI_SIGNING_PRIVATE_KEY_PASSWORD
	(cd "$ROOT" && "$@" cargo build --release --locked -p maxd -p maxhq-vm)
	# The updater archive is made in assemble, after the app is signed.
	# The releases repository goes in at build time, so no tracked file names
	# its owner.
	config='{"bundle":{"createUpdaterArtifacts":false}'
	if [ -n "${MAXHQ_RELEASES_REPO:-}" ]; then
		case "$MAXHQ_RELEASES_REPO" in
		*/*) ;;
		*) die "MAXHQ_RELEASES_REPO is owner/name, not $MAXHQ_RELEASES_REPO" ;;
		esac
		updates=${MAXHQ_UPDATE_MANIFEST:-latest.json}
		case "$updates" in
		latest.json | latest-v[0-9].json) ;;
		*) die "MAXHQ_UPDATE_MANIFEST is latest.json or latest-v<n>.json, not $updates" ;;
		esac
		endpoint=https://github.com/$MAXHQ_RELEASES_REPO/releases/latest/download/$updates
		note "Updates come from $endpoint"
		updater="\"endpoints\":[\"$endpoint\"]"
		if [ -n "${MAXHQ_UPDATE_PUBKEY:-}" ]; then
			case "$MAXHQ_UPDATE_PUBKEY" in
			*[!A-Za-z0-9+/=]*) die "MAXHQ_UPDATE_PUBKEY isn't a base64 public key" ;;
			esac
			note "The updater trusts the public key in MAXHQ_UPDATE_PUBKEY"
			updater="$updater,\"pubkey\":\"$MAXHQ_UPDATE_PUBKEY\""
		fi
		config="$config,\"plugins\":{\"updater\":{$updater}}"
	elif [ -n "${MAXHQ_UPDATE_PUBKEY:-}" ]; then
		die "MAXHQ_UPDATE_PUBKEY needs MAXHQ_RELEASES_REPO"
	else
		warn "MAXHQ_RELEASES_REPO isn't set, so this app can't look for updates"
	fi
	config="$config}"
	(cd "$ROOT" && "$@" pnpm --filter ./apps/desktop exec tauri build --bundles app --config "$config")
	[ -d "$app" ] || die "tauri build made no $app"
}

identity=${MAXHQ_SIGN_IDENTITY:-${APPLE_SIGNING_IDENTITY:--}}

# codesign with the identity, and a secure timestamp unless it's ad hoc.
sign() {
	if [ "$identity" = - ]; then
		codesign --force --sign - "$@"
	else
		codesign --force --sign "$identity" --timestamp "$@"
	fi
}

# Which update key signs: the current one, or the next one of a key change.
case "${MAXHQ_UPDATE_KEY:-current}" in
current) key_flag= ;;
next) key_flag=--next ;;
*) die "MAXHQ_UPDATE_KEY is current or next, not $MAXHQ_UPDATE_KEY" ;;
esac
bridge=${MAXHQ_BRIDGE:-}
if [ "$bridge" = 1 ] && [ -n "$key_flag" ]; then
	die "MAXHQ_BRIDGE=1 signs with the current key, so it can't go with MAXHQ_UPDATE_KEY=next"
fi

# Signs a file with the update key into <file>.sig. Arguments after the
# file go to sign.mjs.
update_sign() {
	file=$1
	shift
	if [ "${MAXHQ_UNSIGNED_UPDATES:-}" = 1 ]; then
		warn "MAXHQ_UNSIGNED_UPDATES=1, so $(basename "$file") has no .sig"
		return 0
	fi
	# shellcheck disable=SC2086 # $key_flag is empty or one flag
	node "$RELEASE/sign.mjs" $key_flag "$@" "$file"
}

assemble() {
	[ -d "$app" ] || die "$app doesn't exist; run the build phase first"
	for bin in maxd maxd-vm; do
		[ -f "$target_dir/release/$bin" ] || die "$target_dir/release/$bin doesn't exist; run the build phase first"
	done
	if [ "${MAXHQ_UNSIGNED_UPDATES:-}" != 1 ]; then
		# shellcheck disable=SC2086 # $key_flag is empty or one flag
		node "$RELEASE/sign.mjs" $key_flag --check || die "no usable update key; set MAXHQ_UNSIGNED_UPDATES=1 to build without one"
		if [ "$bridge" = 1 ]; then
			node "$RELEASE/sign.mjs" --next --check || die "MAXHQ_BRIDGE=1 needs the next update key"
		fi
	fi
	if [ "$notarize" = 1 ] && [ "$identity" = - ]; then
		die "--notarize needs a Developer ID identity; Apple won't notarize ad hoc code"
	fi
	version=$(node "$RELEASE/version.mjs" get)
	macos=$app/Contents/MacOS
	resources=$app/Contents/Resources
	if [ "$identity" = - ]; then
		note "Signing ad hoc"
	else
		note "Signing with $identity"
	fi

	# Inner code first, each signed on its own, so its bytes are final before
	# they're hashed. maxd's enrollment check needs the hardened runtime.
	install -m 0755 "$target_dir/release/maxd" "$macos/maxd"
	sign --options runtime --identifier com.maxagenthq.maxd "$macos/maxd"
	install -m 0755 "$target_dir/release/maxd-vm" "$macos/maxd-vm"
	sign --options runtime --identifier com.maxagenthq.maxd-vm \
		--entitlements "$ROOT/crates/vm/maxd-vm.entitlements" "$macos/maxd-vm"

	set -- "maxd-$mac_target=$macos/maxd"
	rm -f "$resources"/maxd-*-unknown-linux-musl "$resources/SHA256SUMS"
	linux=
	for cpu in x86_64 aarch64; do
		name=maxd-$cpu-unknown-linux-musl
		if [ -n "$linux_dir" ] && [ -f "$linux_dir/$name" ]; then
			install -m 0755 "$linux_dir/$name" "$resources/$name"
			linux="$linux $name"
			set -- "$@" "$name=$resources/$name"
		elif [ "$require_linux" = 1 ]; then
			die "$name is missing from ${linux_dir:-the --linux-dir folder}"
		else
			warn "no $name, so the app can't add or update Linux devices on $cpu"
		fi
	done
	if [ -n "$linux" ]; then
		# shellcheck disable=SC2086 # $linux is a list of plain file names
		(cd "$resources" && shasum -a 256 $linux >SHA256SUMS)
	fi

	note "Writing maxd-release.json for $version"
	manifest=$resources/maxd-release.json
	rm -f "$manifest" "$manifest.sig"
	node "$RELEASE/maxd-release.mjs" --version "$version" --out "$manifest" "$@"
	update_sign "$manifest"

	sign --options runtime "$app"
	codesign --verify --deep --strict "$app"
	if [ "$notarize" = 1 ]; then
		sh "$RELEASE/notarize.sh" "$app"
	fi

	mkdir -p "$out"
	dmg=$out/MaxHQ_$arch.dmg
	note "Making $(basename "$dmg")"
	stage=$(mktemp -d)
	ditto "$app" "$stage/MaxHQ.app"
	ln -s /Applications "$stage/Applications"
	rm -f "$dmg"
	# hdiutil fails now and then with "Resource busy" on hosted runners,
	# where XProtect scans the image as it's made. Those runners are thrown
	# away after the job, so it's stopped there; never on your own Mac.
	if [ "${GITHUB_ACTIONS:-}" = true ]; then
		sudo pkill -9 XProtect >/dev/null 2>&1 || true
	fi
	# Not -quiet, so the log shows why a try failed.
	retry 5 20 hdiutil create -volname MaxHQ -srcfolder "$stage" -ov -format UDZO "$dmg"
	rm -rf "$stage"
	if [ "$identity" != - ]; then
		sign "$dmg"
	fi
	if [ "$notarize" = 1 ]; then
		sh "$RELEASE/notarize.sh" "$dmg"
	fi

	archive=$out/MaxHQ_${version}_$arch.app.tar.gz
	note "Making $(basename "$archive")"
	rm -f "$archive" "$archive.sig" "$archive.next.sig"
	# Without COPYFILE_DISABLE, macOS tar adds ._ files for extended attributes.
	COPYFILE_DISABLE=1 tar -czf "$archive" -C "$(dirname "$app")" MaxHQ.app
	# The signature names the version, and the app's updater installs the
	# archive only as that version.
	update_sign "$archive" --app-version "$version"
	if [ "$bridge" = 1 ] && [ "${MAXHQ_UNSIGNED_UPDATES:-}" != 1 ]; then
		node "$RELEASE/sign.mjs" --next --app-version "$version" --out "$archive.next.sig" "$archive"
	fi

	note "Done"
	echo "  app      $app"
	echo "  dmg      $dmg"
	echo "  updater  $archive"
}

case "$phase" in
build) build ;;
assemble) assemble ;;
all)
	build
	assemble
	;;
esac
