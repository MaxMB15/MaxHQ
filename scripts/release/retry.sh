# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# Sourced by the release scripts, not run on its own.
# shellcheck shell=sh

# Runs a command until it succeeds, up to <attempts> times. Each wait is
# <delay> seconds longer than the last, so `retry 5 20` waits 20, 40, 60 and
# 80 seconds. When every try fails it returns the last try's exit status.
#
# Usage: retry <attempts> <delay> <command> [args...]
retry() {
	_attempts=$1
	_delay=$2
	shift 2
	_attempt=1
	while :; do
		_status=0
		"$@" || _status=$?
		[ "$_status" -eq 0 ] && return 0
		[ "$_attempt" -ge "$_attempts" ] && return "$_status"
		_wait=$((_attempt * _delay))
		echo "'$*' failed, attempt $_attempt of $_attempts. Trying again in ${_wait}s..." >&2
		sleep "$_wait"
		_attempt=$((_attempt + 1))
	done
}
