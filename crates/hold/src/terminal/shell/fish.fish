# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# MaxHQ's hooks for fish in the drawer, sourced (--init-command) once fish
# has read the user's config. They mark prompts and commands and print
# nothing you see.

set -g _maxhq_nonce $MAXHQ_SHELL_NONCE
set -e MAXHQ_SHELL_NONCE
set -g _maxhq_ran 0

function _maxhq_mark
    builtin printf '\e]133;%s;maxhq=%s\a' $argv[1] $_maxhq_nonce
end

# The folder, as OSC 7, and where the prompt starts. A folder name with a
# control character in it can't go.
function _maxhq_prompt --on-event fish_prompt
    if not string match -qr '[[:cntrl:]]' -- $PWD
        builtin printf '\e]7;file://%s%s\a' $hostname (string replace -a '%' '%25' -- $PWD)
    end
    _maxhq_mark A
end

function _maxhq_preexec --on-event fish_preexec
    set -g _maxhq_ran 1
    _maxhq_mark C
end

function _maxhq_postexec --on-event fish_postexec
    set -l ret $status
    if test $_maxhq_ran = 1
        set -g _maxhq_ran 0
        _maxhq_mark "D;$ret"
    end
end

# Where what you type starts, after the user's prompt.
if test -n "$_maxhq_nonce"; and functions -q fish_prompt
    functions -c fish_prompt _maxhq_user_prompt
    function fish_prompt
        _maxhq_user_prompt
        _maxhq_mark B
    end
end
