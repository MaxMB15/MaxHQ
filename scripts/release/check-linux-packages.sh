#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# Checks the Linux app's packages in a Tauri bundle folder, as CI's
# desktop-linux job and the release's build-linux job both pack them.
#
#   bash scripts/release/check-linux-packages.sh <bundle dir>
#
# The bundle folder is target/debug/bundle or target/<triple>/release/bundle,
# holding deb/, rpm/ and appimage/. Each package must carry maxd and name
# WebKitGTK 4.1 and the SSH client, which the app needs to reach other
# devices. Tauri adds WebKitGTK and GTK itself, as a deb package and as the
# libraries an rpm needs, so tauri.linux.conf.json names only the SSH client.
# The desktop entry must be MaxHQ.desktop, the name the dock's launcher count
# is sent to, and name the window's class, so the dock puts the window under
# the app's icon. It needs dpkg-deb and rpm.

set -euo pipefail

fail() {
	echo "${GITHUB_ACTIONS:+::error::}check-linux-packages.sh: $1" >&2
	exit 1
}

[[ $# -eq 1 ]] || fail "usage: check-linux-packages.sh <bundle dir>"
bundle=$1
shopt -s nullglob
deb=("$bundle"/deb/*.deb)
rpm=("$bundle"/rpm/*.rpm)
[[ ${#deb[@]} -eq 1 ]] || fail "$bundle/deb has ${#deb[@]} .deb files, not one."
[[ ${#rpm[@]} -eq 1 ]] || fail "$bundle/rpm has ${#rpm[@]} .rpm files, not one."

echo "deb package: $(dpkg-deb --field "${deb[0]}" Package) ($(dpkg-deb --field "${deb[0]}" Architecture))"
depends="$(dpkg-deb --field "${deb[0]}" Depends)"
echo "deb depends: $depends"
for name in libwebkit2gtk-4.1-0 openssh-client; do
	grep -qw -- "$name" <<<"$depends" || fail "The .deb doesn't depend on $name."
done
# The deb names its files from the root without a leading ./ or /, so the
# names are matched with those taken off.
files="$(dpkg-deb --fsys-tarfile "${deb[0]}" | tar -t | sed -E 's#^\.?/##')"
echo "deb programs:"
grep '^usr/bin/' <<<"$files" || true
grep -qx 'usr/bin/maxd' <<<"$files" || fail "The .deb has no maxd."
grep -qx 'usr/share/applications/MaxHQ.desktop' <<<"$files" ||
	fail "The .deb has no MaxHQ.desktop."
entry="$(dpkg-deb --fsys-tarfile "${deb[0]}" |
	tar -xO --wildcards '*usr/share/applications/MaxHQ.desktop')"
echo "$entry"
grep -q '^StartupWMClass=' <<<"$entry" || fail "MaxHQ.desktop names no window class."

requires="$(rpm -qpR "${rpm[0]}")"
echo "rpm requires: $requires"
for name in 'libwebkit2gtk-4.1.so.0()(64bit)' openssh-clients; do
	grep -qF -- "$name" <<<"$requires" || fail "The .rpm doesn't require $name."
done
files="$(rpm -qpl "${rpm[0]}")"
grep -qx '/usr/bin/maxd' <<<"$files" || fail "The .rpm has no maxd."

# The app copies maxd out of the AppImage from beside its own program, so
# the AppImage must carry it there.
[[ -x "$bundle/appimage/MaxHQ.AppDir/usr/bin/maxd" ]] ||
	fail "The AppImage has no maxd beside the app."
echo "The .deb, the .rpm and the AppImage carry maxd."
