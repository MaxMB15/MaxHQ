#!/bin/sh
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# Signs the release's Android build with the upload key: the unsigned APK
# and the App Bundle the build job made, put back where Gradle wrote them,
# signed by `mobile.ts android-sign`, and written to the out folder under
# their release names. It needs Node, the Android SDK's build-tools and a
# JDK, and no packages.
#
#   sh scripts/release/sign-android.sh <build dir> <out dir>
#
# <build dir> holds app-release-unsigned.apk and app-release.aab. The key is
# in ANDROID_KEYSTORE_BASE64, ANDROID_KEYSTORE_PASSWORD, ANDROID_KEY_ALIAS
# and ANDROID_KEY_PASSWORD. With none of them set it says so and writes
# nothing, so the release carries no Android build until the owner adds the
# key. Half a key fails.

set -eu

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
OUTPUTS=apps/mobile/android/app/build/outputs

die() {
	if [ -n "${GITHUB_ACTIONS:-}" ]; then
		echo "::error::sign-android.sh: $*" >&2
	else
		echo "sign-android.sh: $*" >&2
	fi
	exit 1
}

[ $# -eq 2 ] || die "usage: sign-android.sh <build dir> <out dir>"
build=$1
out=$2
for file in app-release-unsigned.apk app-release.aab; do
	[ -f "$build/$file" ] || die "$build has no $file"
done

if [ -z "${ANDROID_KEYSTORE_BASE64:-}${ANDROID_KEYSTORE_PASSWORD:-}${ANDROID_KEY_ALIAS:-}${ANDROID_KEY_PASSWORD:-}" ]; then
	message="There's no Android upload key, so the APK and the App Bundle stay unsigned and this release carries no Android build. .github/RELEASING.md says where the key goes."
	if [ -n "${GITHUB_ACTIONS:-}" ]; then
		echo "::warning::$message"
		{
			echo "### Android build unsigned"
			echo "$message"
		} >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
	else
		echo "warning: $message"
	fi
	exit 0
fi
# Half a key fails here, before anything is written into the checkout.
# mobile.ts checks the rest.
unset_names=""
for name in ANDROID_KEYSTORE_BASE64 ANDROID_KEYSTORE_PASSWORD ANDROID_KEY_ALIAS ANDROID_KEY_PASSWORD; do
	eval "value=\${$name:-}"
	# shellcheck disable=SC2154 # eval sets value
	[ -n "$value" ] || unset_names="$unset_names $name"
done
[ -z "$unset_names" ] || die "signing needs all four ANDROID_KEY* secrets, and these aren't set:$unset_names"

mkdir -p "$ROOT/$OUTPUTS/apk/release" "$ROOT/$OUTPUTS/bundle/release" "$out"
rm -f "$ROOT/$OUTPUTS/apk/release/app-release.apk"
cp "$build/app-release-unsigned.apk" "$ROOT/$OUTPUTS/apk/release/app-release-unsigned.apk"
cp "$build/app-release.aab" "$ROOT/$OUTPUTS/bundle/release/app-release.aab"
(cd "$ROOT/apps/mobile" && node --disable-warning=MODULE_TYPELESS_PACKAGE_JSON scripts/mobile.ts android-sign)
cp "$ROOT/$OUTPUTS/apk/release/app-release.apk" "$out/MaxHQ.apk"
cp "$ROOT/$OUTPUTS/bundle/release/app-release.aab" "$out/MaxHQ.aab"
ls -l "$out/MaxHQ.apk" "$out/MaxHQ.aab"
