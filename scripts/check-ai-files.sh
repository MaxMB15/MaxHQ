#!/bin/sh
# Fails when the index holds files that .gitignore excludes, such as agent
# instructions and settings (CLAUDE.md, AGENTS.md, .claude/) and the docs AI
# tools wrote. git add -f gets them past .gitignore, so the pre-commit hook
# and CI both run this.

ignored=$(git ls-files --cached --ignored --exclude-standard)
[ -z "$ignored" ] && exit 0

echo "These files are excluded by .gitignore and can't be committed:" >&2
echo "$ignored" | sed 's/^/  /' >&2
echo "Unstage them and keep your copy with: git rm --cached <file>" >&2
exit 1
