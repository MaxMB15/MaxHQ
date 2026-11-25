#!/bin/sh
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# Fails before a release signs anything when an update key it signs with is
# missing, its password is wrong, or it isn't the private half of the key
# the app trusts. sign-macos and sign-desktop both run it from the checkout.
#
#   sh scripts/release/check-update-keys.sh
#
# prepare-release's outputs come from the environment: SIGN_KEY (current or
# next), KEY_STATE (bridge during a key change) and NEXT_PUBKEY. The keys
# come from TAURI_SIGNING_PRIVATE_KEY and the rest (see sign.mjs).

set -eu

RELEASE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$RELEASE/../.." && pwd)

if [ "$SIGN_KEY" = current ]; then
	current=$(node -p "require('$ROOT/apps/desktop/src-tauri/tauri.conf.json').plugins.updater.pubkey")
	node "$RELEASE/sign.mjs" --check --expect "$current"
fi
if [ "$KEY_STATE" = bridge ] || [ "$SIGN_KEY" = next ]; then
	node "$RELEASE/sign.mjs" --next --check --expect "$NEXT_PUBKEY"
fi
