# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# MaxHQ's .zprofile: the user's, as .zshenv here says.

_maxhq_user
if [[ -r ${ZDOTDIR:-$HOME}/.zprofile ]]; then
  builtin source -- "${ZDOTDIR:-$HOME}/.zprofile"
fi
_maxhq_ours
