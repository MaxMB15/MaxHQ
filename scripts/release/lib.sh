# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# Sourced by the release scripts, not run on its own. The script that
# sources it sets RELEASE to this folder first.
# shellcheck shell=sh

# Stops the script with a message on stderr, as an error annotation on
# GitHub Actions. The message starts with the script's name.
die() {
	echo "${GITHUB_ACTIONS:+::error::}$(basename "$0"): $*" >&2
	exit 1
}

# A warning on stderr, as a warning annotation on GitHub Actions, so stdout
# stays free for what a script prints for the next step.
warn() {
	if [ -n "${GITHUB_ACTIONS:-}" ]; then
		echo "::warning::$*" >&2
	else
		echo "warning: $*" >&2
	fi
}

# Which update key signs, from the environment: the current one, or with
# MAXHQ_UPDATE_KEY=next the next one of a key change. Sets key_flag to
# nothing or --next, for sign.mjs. MAXHQ_BRIDGE=1 signs with the current key
# and the next one too, and sets bridge to 1.
choose_update_key() {
	case "${MAXHQ_UPDATE_KEY:-current}" in
	current) key_flag= ;;
	next) key_flag=--next ;;
	*) die "MAXHQ_UPDATE_KEY is current or next, not $MAXHQ_UPDATE_KEY" ;;
	esac
	bridge=${MAXHQ_BRIDGE:-}
	if [ "$bridge" = 1 ] && [ -n "$key_flag" ]; then
		die "MAXHQ_BRIDGE=1 signs with the current key, so it can't go with MAXHQ_UPDATE_KEY=next"
	fi
}

# Fails before anything is signed when a key choose_update_key picked
# doesn't read. $1 is the message for the one key_flag names.
check_update_keys() {
	# shellcheck disable=SC2086 # $key_flag is empty or one flag
	node "$RELEASE/sign.mjs" $key_flag --check || die "$1"
	if [ "$bridge" = 1 ]; then
		node "$RELEASE/sign.mjs" --next --check || die "MAXHQ_BRIDGE=1 needs the next update key"
	fi
}
