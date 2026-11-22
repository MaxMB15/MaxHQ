# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# MaxHQ's startup file for bash in the drawer, which bash reads in place of
# ~/.bashrc (--init-file). It runs what a login shell would, then adds the
# hooks that mark prompts and commands, wrapping the user's own. None of it
# prints anything you see. It keeps to what bash 3.2 has.

_maxhq_nonce=${MAXHQ_SHELL_NONCE-}
_maxhq_pager_flag=${MAXHQ_SHELL_PAGER-}
unset MAXHQ_SHELL_NONCE MAXHQ_SHELL_PAGER

if [ -r /etc/profile ]; then
  . /etc/profile
fi
if [ -r ~/.bash_profile ]; then
  . ~/.bash_profile
elif [ -r ~/.bash_login ]; then
  . ~/.bash_login
elif [ -r ~/.profile ]; then
  . ~/.profile
fi

_maxhq_mark() {
  builtin printf '\033]133;%s;maxhq=%s\007' "$1" "$_maxhq_nonce"
}

# The folder, as OSC 7. A name with a control character in it can't go.
_maxhq_cwd() {
  case $PWD in
    *[[:cntrl:]]*) return 0 ;;
  esac
  builtin printf '\033]7;file://%s%s\007' "$HOSTNAME" "${PWD//\%/%25}"
}

# No pager for a command MaxHQ sends, while the file says so: its card
# can't page. Each value comes back at the next prompt, unless the command
# changed it.
_maxhq_no_pager() {
  if [ -z "$_maxhq_pager_flag" ] || [ ! -e "$_maxhq_pager_flag" ]; then
    return 0
  fi
  _maxhq_paged=1
  _maxhq_pager_set=${PAGER+1}
  _maxhq_pager_was=${PAGER-}
  _maxhq_git_pager_set=${GIT_PAGER+1}
  _maxhq_git_pager_was=${GIT_PAGER-}
  export PAGER=cat GIT_PAGER=cat
}

_maxhq_pager_back() {
  [ "$_maxhq_paged" = 1 ] || return 0
  _maxhq_paged=0
  if [ "${PAGER-}" = cat ]; then
    if [ -n "$_maxhq_pager_set" ]; then PAGER=$_maxhq_pager_was; else unset PAGER; fi
  fi
  if [ "${GIT_PAGER-}" = cat ]; then
    if [ -n "$_maxhq_git_pager_set" ]; then GIT_PAGER=$_maxhq_git_pager_was; else unset GIT_PAGER; fi
  fi
}

# First at each prompt: where the command that ran ended, with its exit
# code, then the folder.
_maxhq_prompt_start() {
  local ret=$?
  _maxhq_waiting=0
  if [ "$_maxhq_ran" = 1 ]; then
    _maxhq_ran=0
    _maxhq_mark "D;$ret"
  fi
  _maxhq_pager_back
  _maxhq_cwd
  return $ret
}

# Last at each prompt, once any theme has set PS1: marks where the prompt
# starts and where what you type starts.
_maxhq_prompt_end() {
  local ret=$?
  if [ "$PS1" != "$_maxhq_ps1" ]; then
    _maxhq_user_ps1=$PS1
    _maxhq_ps1="\[\033]133;A;maxhq=$_maxhq_nonce\007\]$PS1\[\033]133;B;maxhq=$_maxhq_nonce\007\]"
    PS1=$_maxhq_ps1
  fi
  if [ -z "$_maxhq_numbered" ]; then
    # The first prompt, with the history read: no line from before counts
    # as one you typed.
    _maxhq_typed
    _maxhq_numbered=${_maxhq_numbered:-0}
  fi
  _maxhq_waiting=1
  return $ret
}

# The line in $1 as the start mark carries it, in _maxhq_line: its first
# 501 characters, with `%`, `;`, control characters and every byte past
# ASCII percent-encoded, so nothing in it can end the mark.
_maxhq_encode() {
  local line=${1:0:501}
  case $line in
    *[!\ -~]* | *[%\;]*) ;;
    *)
      _maxhq_line=$line
      return 0
      ;;
  esac
  local LC_ALL=C
  local i=0 c out= hex
  while [ "$i" -lt "${#line}" ]; do
    c=${line:$i:1}
    case $c in
      [%\;])
        builtin printf -v hex '%%%02X' "'$c"
        out=$out$hex
        ;;
      [\ -~]) out=$out$c ;;
      *)
        builtin printf -v hex '%d' "'$c"
        builtin printf -v hex '%%%02X' $(((hex + 256) % 256))
        out=$out$hex
        ;;
    esac
    i=$((i + 1))
  done
  _maxhq_line=$out
}

# The newest line in the history, as `history 1` prints it.
_maxhq_newest() {
  HISTTIMEFORMAT= builtin history 1
}

# The line you typed, encoded in _maxhq_line, when the history took it in
# since the last one. The mark carries no text for a line that starts with
# a space, as it keeps out of the history, nor when bash took none in: one
# it ignores as a repeat, or any with the history off.
_maxhq_typed() {
  _maxhq_line=
  local newest number
  newest=$(_maxhq_newest)
  newest=${newest#"${newest%%[![:space:]]*}"}
  number=${newest%%[![:digit:]]*}
  [ -n "$number" ] && [ "$number" != "$_maxhq_numbered" ] || return 0
  _maxhq_numbered=$number
  # The number, a space or a * for a line you changed, a space, the line.
  newest=${newest#"$number"}
  newest=${newest:2}
  case $newest in
    [[:space:]]*) return 0 ;;
  esac
  _maxhq_encode "$newest"
}

# Where the command starts, before the first thing bash runs from what you
# typed, with the line, and PS1 back as it was. A command that starts with
# exec gets no mark: a shell that takes this one's place never marks its
# end, so the command would seem to run for good.
_maxhq_command() {
  local line=${1-}
  _maxhq_waiting=0
  if [ "$PS1" = "$_maxhq_ps1" ]; then
    PS1=$_maxhq_user_ps1
  fi
  _maxhq_no_pager
  line=${line#"${line%%[![:space:]]*}"}
  case $line in
    exec | exec[[:space:]]*) return 0 ;;
  esac
  _maxhq_ran=1
  _maxhq_typed
  _maxhq_mark "C;$_maxhq_line"
}

# The DEBUG trap's part: bash runs it before every command, so it skips
# all but the first from a line you typed. Completion, key bindings,
# functions and, with set -T, subshells and $(...) in PS1 run commands
# too. It returns its first argument, which with extdebug says whether
# the command runs. It keeps $_ as it was, its second.
_maxhq_debug() {
  _maxhq_last=${2-}
  if [ "$_maxhq_waiting" = 1 ] && [ "${BASH_SUBSHELL-0}" = 0 ] &&
    [ "${FUNCNAME[1]-main}" = main ] &&
    [ -z "${COMP_LINE-}" ] && [ -z "${READLINE_LINE-}" ]; then
    case $BASH_COMMAND in
      _maxhq_prompt_*) ;;
      *) _maxhq_command "$BASH_COMMAND" ;;
    esac
  fi
  return "$1"
}

# Returns its first argument. Called last in the trap with $_ as it was
# last, it gives that back to the command.
_maxhq_status() {
  return "$1"
}

# Takes the command out of what `trap -p` prints.
_maxhq_take() {
  _maxhq_user_debug=$2
}

if [ -n "$_maxhq_nonce" ]; then
  _maxhq_ran=0
  _maxhq_waiting=0
  _maxhq_numbered=
  _maxhq_line=
  _maxhq_paged=0
  _maxhq_ps1=
  _maxhq_user_ps1=
  if [ -n "${bash_preexec_imported-}${__bp_imported-}" ]; then
    # bash-preexec owns the trap and PROMPT_COMMAND, and calls these.
    precmd_functions=(_maxhq_prompt_start "${precmd_functions[@]}" _maxhq_prompt_end)
    preexec_functions=("${preexec_functions[@]}" _maxhq_command)
  else
    if [ "${#PROMPT_COMMAND[@]}" -gt 1 ]; then
      PROMPT_COMMAND=(_maxhq_prompt_start "${PROMPT_COMMAND[@]}" _maxhq_prompt_end)
    else
      PROMPT_COMMAND=$'_maxhq_prompt_start\n'"${PROMPT_COMMAND-}"$'\n_maxhq_prompt_end'
    fi
    _maxhq_user_debug=
    _maxhq_trap=$(trap -p DEBUG)
    if [ -n "$_maxhq_trap" ]; then
      eval "_maxhq_take${_maxhq_trap#trap}"
    fi
    unset _maxhq_trap
    # Each ends with $_ as it was as its last argument, so the command
    # sees it unchanged. bash gives the command $? as it was before the
    # trap, and the user's trap gets it too. With extdebug, what the trap
    # returns says whether the command runs: the user's trap decides, and
    # without one every command runs.
    if [ -n "$_maxhq_user_debug" ]; then
      trap '_maxhq_debug "$?" "$_"; eval "$_maxhq_user_debug"; _maxhq_status "$?" "$_maxhq_last"' DEBUG
    else
      trap '_maxhq_debug 0 "$_"' DEBUG
    fi
  fi
fi
