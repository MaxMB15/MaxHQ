<!--
SPDX-License-Identifier: MIT
Copyright (c) 2026 Max Boksem
-->

# Terminals and ! commands

How to open a terminal beside an agent, run commands from the composer with ! so the agent can see them, keep output private, and pick folders on any device.

## Open the terminal panel
<!-- id: terminals/drawer -->
<!-- covers: shortcut/show-or-hide-the-terminal-panel -->

The terminal panel sits under an agent's composer. Press `` ⌃` `` to show or hide it. It opens a shell in the agent's folder, on the device the agent runs on.

1. Open an agent and press `` ⌃` ``.
2. The bar names the folder and the device the shell runs on.
3. Drag the panel's top edge to resize it. Double-click the edge for the default height.
4. Click Hide, or press `` ⌃` `` again, to put it away. The shells keep running.

These open the panel on the Agent shell too, and start it when there's none yet:

- Open in ▾ in the agent's header lists Terminal, "The agent's shell" in its folder on its device.
- The ⋯ menu in the header starts with Open terminal.
- In shell mode, click `` ⌃` `` full terminal at the end of the hints.
- Click the Agent shell pill in the composer.

While the panel is hidden and one of its tabs still runs a command, a line under the composer says so, such as "Terminal · pnpm dev running · 2 tabs". Click it to open the panel on that tab.

A device whose maxd is too old says "Terminals need a newer maxd on" it, and shows none of these.

## Use tabs, splits and panes
<!-- id: terminals/tabs -->

The panel holds several terminals as tabs. The first tab, Agent shell, is the shell your ! commands share.

- New terminal opens another shell after it.
- Split shows two tabs side by side.
- Open as pane moves your terminals out of the panel into panes. The panel then reads "Your terminals are open as panes." A pane has Back to the panel.
- Double-click a tab, or press F2 on it, to Rename it.
- Closing a tab that still runs a command asks first, such as "Stop pnpm dev and close the tab?"
- Press `⌘F` in a terminal to Find in the terminal, with Next match and Previous match. Off a Mac, press `Ctrl+Shift+F`. The key is fixed: MaxHQ has no setting that changes its keys.

Off a Mac, only keys with Ctrl and Shift held go to MaxHQ from a terminal, so Ctrl+F reaches the shell.

When a shell exits, its tab shows the exit code and Restart.

## Jump between commands and reuse their output
<!-- id: terminals/command-marks -->
<!-- covers: shortcut/jump-to-the-previous-or-next-command-in-a-terminal -->

With shell integration on, each command in a terminal gets a mark beside it that shows whether it's running, passed or failed. Click a mark to use that command.

- Send to agent adds what it printed to the agent's composer as an Output pill.
- Copy output and Copy command copy it.
- Press `⌘↑` and `⌘↓` to jump to the previous or next command. Off a Mac, use Control+Shift+Up and Down.

A shell without shell integration shows no marks.

## Send terminal output to the agent
<!-- id: terminals/send-to-agent -->

Select text in any terminal tab and click Send to agent, or right-click it for Send to agent and Copy. Press `⌘↩` to do the same. Off a Mac, press Control+Shift+Return.

The text goes to the agent's composer as a pill named after the tab, with its lines, such as "pnpm dev · 3 lines".

With nothing selected, Send to agent takes the output of the last command the shell marked, such as on the Agent shell tab, or the last lines of a command you ran on its own. Otherwise it's dimmed.

## Run a command from the composer with !
<!-- id: terminals/shell-mode -->

Type ! at the start of the composer to run a shell command instead of sending a message. Its output goes to the agent with your next message.

1. Type ! at the very start. The composer shows a Shell tag and Send becomes Run.
2. Type the command, such as pnpm test, and press ↩.
3. The command runs in the Agent shell, so cd and export carry over to the next one.
4. Press ⌥↩ instead to run it in a fresh shell of its own.

The hints under the composer read "↩ run · ⌥↩ fresh shell · !! private · ⌃` full terminal". Click the last one to open the terminal panel on the Agent shell.

Press ↑ and ↓ in shell mode to reach commands you ran before. Backspace at the start steps back one ! at a time. The folder chip lets you Pick the folder the Agent shell runs in.

## Keep a command's output private with !!
<!-- id: terminals/private -->

Type !! at the start of the composer to run a command the agent doesn't see. The tag reads Private, with "The agent won't see the output".

1. Type !! and the command, then press ↩.
2. The result starts folded and stays out of your next message.
3. To share it later, click Attach to next message on a single run, or switch a block on the Agent shell card from Private to Shared.

Use it for commands that print secrets or long output the agent doesn't need.

## Work with the Agent shell
<!-- id: terminals/session -->

The ! commands you run between two of your messages gather in one card, "Commands you ran on" the device. Each is a block with its output and exit code.

- Each block switches between Shared and Private. A line counts what goes with your next message, such as "1 of 1 go with your next message".
- Send now sends the shared blocks now, on their own.
- Add to next message puts them in the next message you send.
- Open in terminal shows the Agent shell tab in the terminal panel.
- Close shell, in the card's ⋯ menu, closes the shell. While a command still runs, it asks first.

The composer's pill reads Agent shell, or such as "Agent shell · 1 failed". Click it to open the Agent shell in the terminal panel.

Long output folds to its last 20 lines, with Show all lines. A message takes only the last lines of each block. After a cd, the card shows the folder it's now in.

## Commands you type in the Agent shell's tab
<!-- id: terminals/typed -->

A command you type yourself in the Agent shell's tab also lands in the card, as a line such as `$ ls -la` marked "typed in the terminal".

- It starts Private, so the agent sees it only once you switch it to Shared.
- Click the line to show what it printed.
- More than three in a row fold into one line, such as "4 commands typed in the terminal", with how many are shared. Click it to see each one.
- It brings no unread mark and no notification.
- A value set for a name that reads like a secret shows as •••, such as `export GH_TOKEN=•••` or `--password •••`.

Start a command with a space to keep it out of the card, the way shells keep such a line out of their history. Bash records a line only while its history is on, and leaves out one its history skips, such as a repeat under HISTCONTROL=ignoredups. Nothing is recorded from a shell you start inside the Agent shell, or on a device whose maxd is too old.

## Programs that take the whole screen
<!-- id: terminals/full-screen -->

A ! command that takes the whole screen, such as vim or git commit opening your editor, can't run in a card. The card says "In the terminal", with "vim is open in the terminal below. The result comes back here when it closes."

The terminal panel opens on it by itself when all of these hold:

- You ran the command from this window, and it's the newest you ran here.
- It took the whole screen within 10 seconds of starting, for the first time.
- You weren't typing in a field or in another agent's composer. This agent's composer is fine.

A command in the Agent shell opens the Agent shell's tab. One that ran on its own moves to a tab of its own. Otherwise click Open in terminal, which shows only while the panel doesn't show the program. On the phone, the line reads "In the terminal · Open on the Mac".

## Pick from a list in a card
<!-- id: terminals/pickers -->

Some commands ask you to pick with the arrow keys, such as `pnpm create vite`. While one waits, its card says "Waiting for keys".

1. Click the command's output. A ring shows the card now has your keys.
2. Press ↑, ↓ and ↩ to pick. Letters, Space, Tab and Esc go to the program too, and Ctrl-C stops it.
3. Click outside, or press `⌘.`, to give your keys back. Off a Mac, press `Ctrl+.`. They also come back by themselves once the program stops reading keys.

The row under the output reads "↑ ↓ ↩ go to the program", with Open in terminal and Stop. A device whose maxd is too old shows the output without taking keys. On the phone, the line reads "Waiting for keys · Open on the Mac".

To type a secret into a card's reply line, click its lock, Hide what you type, first. The device then keeps the reply's echo out of the card and notes only that it went, as it does for the password field. A device whose maxd is too old gets the reply as plain input, so a program that echoes what you type shows it.

## Why a command ran on its own
<!-- id: terminals/fresh-shell -->

Some ! commands run in a fresh shell instead of the Agent shell. The result says why in one line.

- "Sessions need Shell integration, which is off on" the device. Turn it on in that device's Terminals card.
- "Your shell didn't report its commands": the shell gave MaxHQ no marks.
- "The session was busy": a command still ran there.
- "This spans more than one line" or "This starts with exec": the Agent shell takes neither.

A command you run with ⌥↩ runs on its own without a note.

## Work with a command that ran on its own
<!-- id: terminals/you-ran -->

A command that ran on its own shows as a "You ran" card in the conversation.

- While it runs it shows the live terminal and a line to type into it, with Stop, then Kill if it keeps going.
- Open in terminal hands it to a tab in the terminal panel and leaves a line that links to it.
- A command that asks for a password shows "Waiting for your password", with a secure field. What you type goes only to the terminal.

Once it ends:

- Show output and Hide output fold it.
- Ask about this puts "Look at this output." in the composer.
- Run again, Copy command, and Keep private or Attach to next message.

## Run a code block from an agent's reply
<!-- id: terminals/run-code-block -->

A shell code block in an agent's reply, such as one marked bash, zsh, sh or fish, has a Run menu. Nothing runs from the menu itself, so you can read the command first.

- Run, with "The agent sees the output", puts the command in the composer after !, ready to edit. Press ↩ to run it.
- Run privately, with "The agent doesn't see it", does the same after !!.
- Paste into terminal types it into the terminal panel without pressing Enter.

A command the agent ran in its shell has the same menu once it ends. Run it yourself puts it in the composer with !, ready to edit, and Paste into terminal types it into the panel.

The menu shows only on agents that take messages, on devices with terminals.

## Turn shell integration on or off
<!-- id: terminals/shell-integration -->
<!-- covers: settings/device/terminals -->

Shell integration lets MaxHQ tell your commands apart in zsh, bash and fish, and on Windows in Git Bash and PowerShell. It's on by default, in the Terminals card on each device's tab in Settings.

1. Switch the sidebar to Devices and open the device.
2. Find the Terminals card and turn Shell integration on or off.

With it on, commands get their marks and ! commands share one session. Off starts shells exactly as your startup files say, with no marks, and each ! command runs in a fresh shell. A change applies to terminals opened after it. See [Devices and links](devices.md).

## Type or browse to a folder on any device
<!-- id: terminals/path-picker -->

Folder fields, such as Folder in New agent, suggest paths from the device you picked as you type.

1. Start typing a path, such as ~/code/example-app. The list reads "Tab completes · ↩ picks · on" the device.
2. Press Tab to complete, ↩ to pick, or Esc to close the list.
3. Click Browse… to look instead. On this Mac it opens the system's folder picker. On another device it opens MaxHQ's folder browser, with Up, Show hidden, New folder and Choose.

Drop a Finder folder on another device's field and MaxHQ says "That folder is on this Mac." If that device has a checkout of the same repo, click Use that. An older maxd says to update it to browse its folders.
