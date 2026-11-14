#!/bin/sh
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# Checks that a Developer ID build will open on other Macs. Every Mac binary
# in the app must be signed by the same team with a secure timestamp, and
# executables must use the hardened runtime. With --notarized, the app (and
# the DMG, if given) must also carry a stapled ticket Gatekeeper accepts.
#
#   sh scripts/release/verify-signing.sh [--notarized] <MaxHQ.app> [dmg]

set -eu

. "$(dirname "$0")/retry.sh"

notarized=0
if [ "${1:-}" = --notarized ]; then
	notarized=1
	shift
fi
app=${1:?usage: verify-signing.sh [--notarized] <app> [dmg]}
dmg=${2:-}

codesign --verify --deep --strict --verbose=2 "$app"

# codesign -d prints to stderr.
signature_of() { codesign -dv --verbose=4 "$1" 2>&1; }
team_of() { signature_of "$1" | sed -n 's/^TeamIdentifier=//p'; }

team=$(team_of "$app")
if [ -z "$team" ] || [ "$team" = "not set" ]; then
	echo "Error: $app isn't signed with a Developer ID certificate" >&2
	exit 1
fi

problems=
count=0
# The bundle's paths have no newlines.
files=$(find "$app/Contents" -type f)
while IFS= read -r f; do
	[ -n "$f" ] || continue
	kind=$(file -b "$f")
	case "$kind" in
	Mach-O*) ;;
	*) continue ;;
	esac
	count=$((count + 1))
	signature=$(signature_of "$f")
	name=${f#"$app"/}
	file_team=$(printf '%s\n' "$signature" | sed -n 's/^TeamIdentifier=//p')
	if [ "$file_team" != "$team" ]; then
		problems="$problems
  $name is signed by team '${file_team:-none}', not $team"
	fi
	if ! printf '%s\n' "$signature" | grep -q '^Timestamp='; then
		problems="$problems
  $name has no secure timestamp"
	fi
	case "$kind" in
	*executable*)
		if ! printf '%s\n' "$signature" | grep -q '^CodeDirectory .*(runtime)'; then
			problems="$problems
  $name doesn't use the hardened runtime"
		fi
		;;
	esac
done <<EOF
$files
EOF

if [ -n "$dmg" ]; then
	codesign --verify --strict --verbose=2 "$dmg"
	dmg_team=$(team_of "$dmg")
	if [ "$dmg_team" != "$team" ]; then
		problems="$problems
  $(basename "$dmg") is signed by team '${dmg_team:-none}', not $team"
	fi
fi

if [ -n "$problems" ]; then
	echo "Error: signing problems in $(basename "$app"):$problems" >&2
	exit 1
fi
echo "All $count binaries in $(basename "$app") are signed by team $team with a timestamp."

if [ "$notarized" = 1 ]; then
	# stapler validate asks Apple's server, so a dropped connection fails it.
	retry 5 20 xcrun stapler validate "$app"
	spctl --assess --type execute --verbose=2 "$app"
	if [ -n "$dmg" ]; then
		retry 5 20 xcrun stapler validate "$dmg"
		spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
	fi
fi
