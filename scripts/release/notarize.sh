#!/bin/sh
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# Notarizes an app or a DMG with Apple and staples the ticket to it. An app
# goes up as a zip, since notarytool doesn't take a folder.
#
#   sh scripts/release/notarize.sh <MaxHQ.app | file.dmg>
#
# Needs the App Store Connect API key, in the variables the macos-signing
# action sets: APPLE_API_KEY (the key's ID), APPLE_API_ISSUER and
# APPLE_API_KEY_PATH (the .p8 file).

set -eu

. "$(dirname "$0")/retry.sh"

file=${1:?usage: notarize.sh <app or dmg>}
: "${APPLE_API_KEY:?}" "${APPLE_API_ISSUER:?}" "${APPLE_API_KEY_PATH:?}"
name=$(basename "$file")

submit=$file
zip=
case "$file" in
*.app)
	zip=$(mktemp -d)/$name.zip
	ditto -c -k --keepParent "$file" "$zip"
	submit=$zip
	;;
esac

echo "Submitting $name for notarization..."
# notarytool exits non-zero for a rejected submission, so read the status.
result=$(xcrun notarytool submit "$submit" --key "$APPLE_API_KEY_PATH" \
	--key-id "$APPLE_API_KEY" --issuer "$APPLE_API_ISSUER" \
	--wait --timeout 1h --output-format json) || true
[ -n "$zip" ] && rm -rf "$(dirname "$zip")"
echo "$result"
status=$(printf '%s' "$result" | jq -r '.status // empty' 2>/dev/null || true)

if [ "$status" != Accepted ]; then
	id=$(printf '%s' "$result" | jq -r '.id // empty' 2>/dev/null || true)
	if [ -n "$id" ]; then
		# The log names each file Apple rejected, and why.
		xcrun notarytool log "$id" --key "$APPLE_API_KEY_PATH" \
			--key-id "$APPLE_API_KEY" --issuer "$APPLE_API_ISSUER" || true
	fi
	echo "Error: notarizing $name ended with status '${status:-unknown}'" >&2
	exit 1
fi

# Stapling downloads the ticket from Apple, so a dropped connection or a
# ticket that isn't published yet fails it.
if ! retry 5 20 xcrun stapler staple "$file"; then
	echo "Error: couldn't staple the notarization ticket to $name" >&2
	exit 1
fi
