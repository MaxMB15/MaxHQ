#!/bin/sh
# Runs from pnpm install. Points git at the hooks in .githooks so the
# AI-files and license checks, the private-names check, formatting and tests
# run before each commit. An install outside this repo's own checkout, such
# as a source tarball, has nothing to set, even when another repo holds that
# folder.

here=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P) || exit 0
top=$(git -C "$here" rev-parse --show-toplevel 2>/dev/null) || exit 0
[ "$top" = "$here" ] || exit 0
git -C "$here" config core.hooksPath .githooks
