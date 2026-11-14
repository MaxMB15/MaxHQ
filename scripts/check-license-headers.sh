#!/bin/sh
# Fails when the index holds a file with the proprietary license MaxHQ had
# before it became MIT licensed: its SPDX id, or a line reserving all rights.
# A branch started before the switch can bring them back when it merges, so
# the pre-commit hook and CI both run this. THIRD_PARTY_NOTICES quotes other
# projects' license texts, which may reserve rights, so only the SPDX id
# counts there. The brackets keep this file from matching itself.

cd "$(git rev-parse --show-toplevel)" || exit 2

id='LicenseRef-MaxHQ-Proprietar[y]'
reserved='[A]ll rights reserved'

# git grep exits 1 when nothing matches and above 1 when it fails.
hits=$(git grep --cached -n -I -i -e "$id" -e "$reserved" -- . ':!THIRD_PARTY_NOTICES') ||
	[ $? -eq 1 ] || exit 2
notices=$(git grep --cached -n -I -i -e "$id" -- THIRD_PARTY_NOTICES) ||
	[ $? -eq 1 ] || exit 2

hits=$(printf '%s\n%s\n' "$hits" "$notices" | sed '/^$/d')
[ -z "$hits" ] && exit 0

echo "These lines carry MaxHQ's old proprietary license:" >&2
echo "$hits" | sed 's/^/  /' >&2
echo "MaxHQ is MIT licensed. Head each file with these two lines instead:" >&2
echo "  SPDX-License-Identifier: MIT" >&2
echo "  Copyright (c) 2026 Max Boksem" >&2
exit 1
