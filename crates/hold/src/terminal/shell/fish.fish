# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# MaxHQ's hooks for fish in the drawer, sourced (--init-command) once fish
# has read the user's config. They mark prompts and commands and print
# nothing you see.

# The vendor snippet took the nonce before the config ran, unless fish
# didn't find it.
if set -q MAXHQ_SHELL_NONCE
    set -g _maxhq_nonce $MAXHQ_SHELL_NONCE
    set -e MAXHQ_SHELL_NONCE
end
if set -q MAXHQ_SHELL_PAGER
    set -g _maxhq_pager_flag $MAXHQ_SHELL_PAGER
    set -e MAXHQ_SHELL_PAGER
end
set -g _maxhq_ran 0
set -g _maxhq_paged 0

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

# No pager for a command MaxHQ sends, while the file says so: its card
# can't page. Each value comes back after the command, unless it changed
# it. A universal value shows again once the global one goes.
function _maxhq_no_pager
    if test -z "$_maxhq_pager_flag"; or not test -e "$_maxhq_pager_flag"
        return
    end
    set -g _maxhq_paged 1
    set -e _maxhq_pager_was
    set -e _maxhq_git_pager_was
    set -q PAGER; and set -g _maxhq_pager_was $PAGER
    set -q GIT_PAGER; and set -g _maxhq_git_pager_was $GIT_PAGER
    set -gx PAGER cat
    set -gx GIT_PAGER cat
end

function _maxhq_pager_back
    test "$_maxhq_paged" = 1; or return
    set -g _maxhq_paged 0
    if test "$PAGER" = cat
        set -eg PAGER
        if set -q _maxhq_pager_was; and not set -q PAGER
            set -gx PAGER $_maxhq_pager_was
        end
    end
    if test "$GIT_PAGER" = cat
        set -eg GIT_PAGER
        if set -q _maxhq_git_pager_was; and not set -q GIT_PAGER
            set -gx GIT_PAGER $_maxhq_git_pager_was
        end
    end
end

# Where the command starts, with the line you typed: its first 501
# characters, percent-encoded so nothing in it can end the mark. A line
# that starts with a space keeps its text to itself, as it keeps out of
# the history. A command that starts with exec gets no mark: a shell that
# takes this one's place never marks its end, so the command would seem to
# run for good.
function _maxhq_preexec --on-event fish_preexec
    _maxhq_no_pager
    if string match -qr '^\s*exec(\s|$)' -- "$argv[1]"
        return
    end
    set -g _maxhq_ran 1
    set -l line ''
    if not string match -qr '^\s' -- "$argv[1]"
        set line (string sub -l 501 -- "$argv[1]" | string collect | string escape --style=url)
    end
    _maxhq_mark "C;$line"
end

function _maxhq_postexec --on-event fish_postexec
    set -l ret $status
    if test $_maxhq_ran = 1
        set -g _maxhq_ran 0
        _maxhq_mark "D;$ret"
    end
    _maxhq_pager_back
end

# Where what you type starts, after the user's prompt.
if test -n "$_maxhq_nonce"; and functions -q fish_prompt
    functions -c fish_prompt _maxhq_user_prompt
    function fish_prompt
        _maxhq_user_prompt
        _maxhq_mark B
    end
end
