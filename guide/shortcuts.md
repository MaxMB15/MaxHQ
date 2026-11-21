<!--
SPDX-License-Identifier: MIT
Copyright (c) 2026 Max Boksem
-->

# Keyboard shortcuts

Every key MaxHQ listens for, by group, and how to see the list in the app. The keys here are a Mac's.

## See every shortcut
<!-- id: shortcuts/list -->
<!-- covers: settings/shortcuts -->

Press `⌘/`, Show keyboard shortcuts, to see the list over any view. Settings has the same list on its Keyboard shortcuts page.

- The groups are General, Agents, Sidebar, Inbox, Panes, Find, Composer and Notes. Other holds a key that has no words yet.
- A key marked Projects only works only in projects mode.
- Off a Mac, `⌘` reads Ctrl, `⌥` reads Alt, `⇧` reads Shift, `⏎` reads Enter, and the arrows read Up and Down. So `⇧⌘D` is `Ctrl+Shift+D` there.
- Show or hide the terminal holds Control everywhere: `` ⌃` ``.

## General
<!-- id: shortcuts/general -->
<!-- covers: settings/shortcuts shortcut/show-keyboard-shortcuts shortcut/open-settings -->

General keys work in any view.

| What it does | Keys |
| --- | --- |
| Show keyboard shortcuts | `⌘/` |
| Open settings | `⌘,` |

## Agents
<!-- id: shortcuts/agents -->
<!-- covers: settings/shortcuts shortcut/new-agent shortcut/open-the-next-agent-waiting-on-you shortcut/start-the-agent-in-the-new-agent-form shortcut/pick-the-main-option-on-an-approval-card-with-focus shortcut/pick-that-option-on-a-question-card-with-focus -->

`⌘N` starts a new agent, and `⌘J` opens the next agent waiting on you. Approval and question cards take keys while they have focus.

| What it does | Keys |
| --- | --- |
| New agent | `⌘N` |
| Open the next agent waiting on you | `⌘J` |
| Start the agent, in the New agent form | `⌘⏎` |
| Pick the main option, on an approval card with focus | `⏎` |
| Pick that option, on a question card with focus | `1` to `9` |

## Sidebar
<!-- id: shortcuts/sidebar -->
<!-- covers: settings/shortcuts shortcut/show-or-hide-the-sidebar shortcut/filter-the-agents shortcut/open-the-first-agent-the-filter-shows shortcut/clear-the-filter-then-leave-it shortcut/resize-the-sidebar-on-its-edge-with-focus shortcut/move-between-the-agents-on-an-agent-with-focus shortcut/open-the-agent-on-an-agent-with-focus -->

`⌘B` shows or hides the sidebar, and `⌥⌘F` filters its agents. The arrows move between agents once one has focus.

| What it does | Keys |
| --- | --- |
| Show or hide the sidebar | `⌘B` |
| Filter the agents | `⌥⌘F` |
| Open the first agent the filter shows | `⏎` |
| Clear the filter, then leave it | `Esc` |
| Resize the sidebar, on its edge with focus | `←` `→` |
| Move between the agents, on an agent with focus | `↑` `↓` |
| Open the agent, on an agent with focus | `⏎` |

## Inbox
<!-- id: shortcuts/inbox -->
<!-- covers: settings/shortcuts shortcut/settle-and-go-to-the-next-agent-waiting-or-settle-the-picked-ones shortcut/pin-or-unpin-the-chosen-agents shortcut/undo-the-last-change-while-its-notice-shows -->

`⇧⌘S` settles an agent and goes to the next one waiting. `⌘Z` undoes the last change while its notice shows.

| What it does | Keys |
| --- | --- |
| Settle and go to the next agent waiting, or settle the picked ones | `⇧⌘S` |
| Pin or unpin the chosen agents | `⇧⌘P` |
| Undo the last change, while its notice shows | `⌘Z` |

## Panes
<!-- id: shortcuts/panes -->
<!-- covers: settings/shortcuts shortcut/move-to-the-pane-on-that-side shortcut/next-tab shortcut/previous-tab shortcut/show-the-tab-at-that-place-9-for-the-last shortcut/split-right shortcut/split-down shortcut/maximize-or-restore-the-pane shortcut/close-the-tab shortcut/reopen-the-last-closed-tab shortcut/show-or-hide-the-terminal shortcut/jump-to-the-previous-or-next-command-in-a-terminal -->

`⌘D` splits the pane right and `⇧⌘D` splits it down. These keys work only in projects mode, except Reopen the last closed tab, which works everywhere.

| What it does | Keys |
| --- | --- |
| Move to the pane on that side | `⌥⌘←` `⌥⌘→` `⌥⌘↑` `⌥⌘↓` |
| Next tab | `⇧⌘]` |
| Previous tab | `⇧⌘[` |
| Show the tab at that place, 9 for the last | `⌘1` to `⌘9` |
| Split right | `⌘D` |
| Split down | `⇧⌘D` |
| Maximize or restore the pane | `⇧⌘M` |
| Close the tab | `⇧⌘W` |
| Reopen the last closed tab | `⇧⌘T` |
| Show or hide the terminal | `` ⌃` `` |
| Jump to the previous or next command, in a terminal | `⌘↑` or `⌘↓` |

Off a Mac, jump between commands with `Ctrl+Shift+Up` or `Ctrl+Shift+Down`.

## Find
<!-- id: shortcuts/find -->
<!-- covers: settings/shortcuts shortcut/find-in-the-conversation shortcut/next-or-previous-match shortcut/close-find -->

`⌘F` finds words in the conversation of the agent in view, or in the terminal that has focus. Off a Mac, Find in the terminal is `Ctrl+Shift+F`, since Ctrl+F belongs to the shell there. These keys work only in projects mode.

| What it does | Keys |
| --- | --- |
| Find in the conversation | `⌘F` |
| Next or previous match | `⏎` or `⇧⏎` |
| Close find | `Esc` |

## Composer
<!-- id: shortcuts/composer -->
<!-- covers: settings/shortcuts shortcut/send-or-queue-while-the-agent-works shortcut/send-at-once-to-steer-the-agent shortcut/new-line shortcut/edit-the-last-queued-message-from-an-empty-composer shortcut/bring-back-an-earlier-message-with-the-caret-at-the-start shortcut/stop-the-agent-s-turn shortcut/attach-files -->

`⏎` sends a message and `⇧⏎` starts a new line. While the agent works, `⏎` queues the message and `⌘⏎` sends it at once to steer.

| What it does | Keys |
| --- | --- |
| Send, or queue while the agent works | `⏎` |
| Send at once to steer the agent | `⌘⏎` |
| New line | `⇧⏎` or `⌥⏎` |
| Edit the last queued message, from an empty composer | `↑` |
| Bring back an earlier message, with the caret at the start | `↑` or `↓` |
| Stop the agent's turn | `⌘.` |
| Attach files | `⌘U` |

With Settings, Editor, While an agent works, Enter set to Steer, the two swap: `⏎` sends, or steers while the agent works, and `⌘⏎` queues until the agent finishes.

## Notes
<!-- id: shortcuts/notes -->
<!-- covers: settings/shortcuts shortcut/move-the-lines-up-or-down-in-a-note shortcut/copy-the-lines-up-or-down-in-a-note -->

In a note, `⌥↑` and `⌥↓` move the lines with the caret or the selection. Add `⇧` to copy them instead.

| What it does | Keys |
| --- | --- |
| Move the lines up or down, in a note | `⌥↑` or `⌥↓` |
| Copy the lines up or down, in a note | `⌥⇧↑` or `⌥⇧↓` |
