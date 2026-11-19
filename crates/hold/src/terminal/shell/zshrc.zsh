# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# MaxHQ's .zshrc: the user's, as .zshenv here says, then the hooks, last
# so they wrap what the user's set up.

_maxhq_user
if [[ -r ${ZDOTDIR:-$HOME}/.zshrc ]]; then
  builtin source -- "${ZDOTDIR:-$HOME}/.zshrc"
fi
_maxhq_ours
_maxhq_hook
