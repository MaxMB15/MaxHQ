# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# MaxHQ's .zshenv for a shell in the drawer. ZDOTDIR points at this folder,
# so zsh reads MaxHQ's startup files, and each runs the user's own in turn
# with ZDOTDIR as the user had it. The last of them leave it that way. The
# hooks that mark prompts and commands go in after the user's .zshrc. None
# of it prints anything you see.

_maxhq_dir=$ZDOTDIR
_maxhq_user_dir=${MAXHQ_USER_ZDOTDIR-}
_maxhq_nonce=${MAXHQ_SHELL_NONCE-}
_maxhq_ran=0
_maxhq_ps1=
_maxhq_user_ps1=
# What goes around PS1, where the prompt starts and where what you type
# starts.
_maxhq_a="%{"$'\e]133;A;maxhq='"$_maxhq_nonce"$'\a'"%}"
_maxhq_b="%{"$'\e]133;B;maxhq='"$_maxhq_nonce"$'\a'"%}"
unset MAXHQ_USER_ZDOTDIR MAXHQ_SHELL_NONCE

# ZDOTDIR as the user has it, while their files run.
_maxhq_user() {
  emulate -L zsh
  if [[ -n $_maxhq_user_dir ]]; then
    export ZDOTDIR=$_maxhq_user_dir
  else
    unset ZDOTDIR
  fi
  # A system file that ran with ZDOTDIR here put the history here too.
  if [[ ${HISTFILE-} == "$_maxhq_dir/.zsh_history" ]]; then
    HISTFILE=${ZDOTDIR:-$HOME}/.zsh_history
  fi
}

# ZDOTDIR back here for zsh's next file, keeping any the user set.
_maxhq_ours() {
  emulate -L zsh
  _maxhq_user_dir=${ZDOTDIR-}
  export ZDOTDIR=$_maxhq_dir
}

# The hooks are defined here, before the user's files, so no alias of
# theirs changes them.

# Puts the hooks in, ahead of and after the user's.
_maxhq_hook() {
  emulate -L zsh
  [[ -n $_maxhq_nonce ]] || return 0
  precmd_functions=(_maxhq_precmd ${precmd_functions:#_maxhq_(precmd|prompt)} _maxhq_prompt)
  preexec_functions=(${preexec_functions:#_maxhq_preexec} _maxhq_preexec)
}

_maxhq_mark() {
  builtin print -rn -- $'\e]133;'"$1;maxhq=$_maxhq_nonce"$'\a'
}

# The folder, as OSC 7. A name with a control character in it can't go.
_maxhq_cwd() {
  emulate -L zsh
  [[ $PWD == *[[:cntrl:]]* ]] && return
  builtin print -rn -- $'\e]7;file://'"$HOST${PWD//\%/%25}"$'\a'
}

# First at each prompt: where the command that ran ended, with its exit
# code, then the folder.
_maxhq_precmd() {
  local ret=$?
  emulate -L zsh
  if (( _maxhq_ran )); then
    _maxhq_ran=0
    _maxhq_mark "D;$ret"
  fi
  [[ ${ZDOTDIR-} == "$_maxhq_dir" ]] && _maxhq_user
  _maxhq_cwd
  return $ret
}

# Last at each prompt, once any theme has set PS1: marks it.
_maxhq_prompt() {
  emulate -L zsh
  [[ -o prompt_percent && $PS1 != "$_maxhq_ps1" ]] || return 0
  _maxhq_user_ps1=$PS1
  _maxhq_ps1=$_maxhq_a$PS1$_maxhq_b
  PS1=$_maxhq_ps1
}

# Last before a command runs: gives PS1 back as it was, keeps the prompt
# hooks first and last, and marks where the command starts. A command
# that starts with exec gets no mark: a shell that takes this one's place
# never marks its end, so the command would seem to run for good.
_maxhq_preexec() {
  emulate -L zsh
  [[ $PS1 == "$_maxhq_ps1" ]] && PS1=$_maxhq_user_ps1
  precmd_functions=(_maxhq_precmd ${precmd_functions:#_maxhq_(precmd|prompt)} _maxhq_prompt)
  local line=${1:-$3}
  line=${line#"${line%%[![:space:]]*}"}
  case $line in
    exec | exec[[:space:]]*) return 0 ;;
  esac
  _maxhq_ran=1
  _maxhq_mark C
}

_maxhq_user
if [[ -r ${ZDOTDIR:-$HOME}/.zshenv ]]; then
  builtin source -- "${ZDOTDIR:-$HOME}/.zshenv"
fi
_maxhq_ours
