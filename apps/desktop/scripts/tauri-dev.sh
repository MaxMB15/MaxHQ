#!/bin/sh
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# `tauri dev` with dev-app.sh as Cargo's runner on macOS, so the app runs
# from a bundle and can show notifications. The runner is set here alone,
# so tests and every other Cargo command run binaries as they are.

set -eu

if [ "$(uname -s)" = "Darwin" ]; then
	runner="$(cd -- "$(dirname -- "$0")" && pwd)/dev-app.sh"
	export CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER="$runner"
	export CARGO_TARGET_X86_64_APPLE_DARWIN_RUNNER="$runner"
fi

exec tauri dev "$@"
