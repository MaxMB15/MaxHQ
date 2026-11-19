# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# MaxHQ's .zlogin, the last startup file: the user's, as .zshenv here
# says, with ZDOTDIR left as the user has it.

_maxhq_user
if [[ -r ${ZDOTDIR:-$HOME}/.zlogin ]]; then
  builtin source -- "${ZDOTDIR:-$HOME}/.zlogin"
fi
