#!/bin/sh
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# Cargo's runner for `pnpm dev`, set by tauri-dev.sh. Cargo calls it with
# the binary it built and that binary's arguments.
#
# Notification Center only works for an app launched from a bundle, so on
# macOS the desktop app runs from "MaxHQ Dev.app" beside the binary in the
# target folder. The bundle has the app's identifier with ".dev" on the
# end, so macOS asks about its notifications and lists it in System
# Settings apart from the app you installed. Each run copies the fresh
# binary in, since Cargo writes a new file on every build, and signs the
# bundle ad hoc. `maxd` stays a link to the one Cargo builds, where the
# app looks for it beside its own binary.
#
# Every other binary, such as a test, runs as it is.

set -eu

binary=${1:?usage: dev-app.sh BINARY [ARGS...]}
name=$(basename -- "$binary")
if [ "$name" != "maxhq-desktop" ] || [ "$(uname -s)" != "Darwin" ]; then
	exec "$@"
fi
shift

here=$(cd -- "$(dirname -- "$0")" && pwd)
tauri="$here/../src-tauri"
target=$(cd -- "$(dirname -- "$binary")" && pwd)
app="$target/MaxHQ Dev.app"
contents="$app/Contents"

# The release identifier from the Tauri config, so the two can't drift.
identifier=$(plutil -extract identifier raw -o - "$tauri/tauri.conf.json" 2>/dev/null ||
	echo com.maxagenthq.maxhq)
identifier="$identifier.dev"
version=${CARGO_PKG_VERSION:-0.0.0}

mkdir -p "$contents/MacOS" "$contents/Resources"

plist="$contents/Info.plist"
fresh="$contents/Info.plist.new"
cat >"$fresh" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleDisplayName</key>
	<string>MaxHQ Dev</string>
	<key>CFBundleExecutable</key>
	<string>$name</string>
	<key>CFBundleIconFile</key>
	<string>icon</string>
	<key>CFBundleIdentifier</key>
	<string>$identifier</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>MaxHQ Dev</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>$version</string>
	<key>CFBundleVersion</key>
	<string>$version</string>
	<key>LSMinimumSystemVersion</key>
	<string>15.0</string>
	<key>NSHighResolutionCapable</key>
	<true/>
</dict>
</plist>
EOF
# Only a change replaces it, so the bundle keeps its date between runs.
if cmp -s "$fresh" "$plist"; then
	rm -f "$fresh"
else
	mv -f "$fresh" "$plist"
fi

icon="$tauri/icons/icon.icns"
if [ -f "$icon" ] && ! cmp -s "$icon" "$contents/Resources/icon.icns"; then
	cp "$icon" "$contents/Resources/icon.icns"
fi

# A clone on APFS costs nothing, and a plain copy works anywhere else.
inside="$contents/MacOS/$name"
rm -f "$inside"
cp -c "$binary" "$inside" 2>/dev/null || cp "$binary" "$inside"
ln -sfn ../../../maxd "$contents/MacOS/maxd"

if ! codesign --force --sign - "$app" >/dev/null 2>&1; then
	echo "dev-app.sh: couldn't sign $app, so notifications may not show" >&2
fi

exec "$inside" "$@"
