# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# MaxHQ's first step for fish in the drawer, which fish finds through
# XDG_DATA_DIRS and runs before the user's config.fish. It takes the nonce
# out of the environment, so nothing the config starts gets it, and puts
# XDG_DATA_DIRS back as the user had it.

if set -q MAXHQ_SHELL_NONCE
    set -g _maxhq_nonce $MAXHQ_SHELL_NONCE
    set -e MAXHQ_SHELL_NONCE
end
if set -q MAXHQ_USER_XDG_DATA_DIRS
    if test -n "$MAXHQ_USER_XDG_DATA_DIRS"
        set -gx XDG_DATA_DIRS $MAXHQ_USER_XDG_DATA_DIRS
    else
        set -e XDG_DATA_DIRS
    end
    set -e MAXHQ_USER_XDG_DATA_DIRS
end
