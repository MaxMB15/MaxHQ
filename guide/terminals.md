<!--
SPDX-License-Identifier: MIT
Copyright (c) 2026 Max Boksem
-->

# Terminals and ! commands

How to open a terminal beside an agent, run commands from the composer with ! so the agent can see them, keep output private, and pick folders on any device.

## Open the terminal drawer
<!-- id: terminals/drawer -->
<!-- covers: shortcut/show-or-hide-the-terminal -->

The terminal drawer sits under an agent's composer. Press `` ⌃` `` to show or hide it. It opens a shell in the agent's folder, on the device the agent runs on.

1. Open an agent and press `` ⌃` ``.
2. The bar names the folder and the device the shell runs on.
3. Drag the drawer's top edge to resize it. Double-click the edge for the default height.
4. Click Hide, or press `` ⌃` `` again, to put it away. The shells keep running.

A device whose maxd is too old says "Terminals need a newer maxd on" it.

## Use tabs, splits and panes
<!-- id: terminals/tabs -->

The drawer holds several terminals as tabs. The first tab, Session, is the shell your ! commands share.

- New terminal opens another shell after it.
- Split shows two tabs side by side.
- Open as pane moves your terminals out of the drawer into panes. The drawer then reads "Your terminals are open as panes." A pane has Back to the drawer.
- Double-click a tab, or press F2 on it, to Rename it.
- Closing a tab that still runs a command asks first, such as "Stop pnpm dev and close the tab?"
- Press `⌘F` in a terminal to Find in the terminal, with Next match and Previous match. Off a Mac, press `Ctrl+Shift+F`. The key is fixed: MaxHQ has no setting that changes its keys.

Off a Mac, only keys with Ctrl and Shift held go to MaxHQ from a terminal, so Ctrl+F reaches the shell.

When a shell exits, its tab shows the exit code and Restart.

## Jump between commands and reuse their output
<!-- id: terminals/command-marks -->
<!-- covers: shortcut/jump-to-the-previous-or-next-command-in-a-terminal -->

With shell integration on, each command in a terminal gets a mark beside it that shows whether it's running, passed or failed. Click a mark to use that command.

- Add to chat adds what it printed to the agent's composer as an Output pill.
- Copy output and Copy command copy it.
- Press `⌘↑` and `⌘↓` to jump to the previous or next command. Off a Mac, use Control+Shift+Up and Down.
- Select text in a terminal and click Add to chat to add only those lines.

A shell without shell integration shows no marks.

## Run a command from the composer with !
<!-- id: terminals/shell-mode -->

Type ! at the start of the composer to run a shell command instead of sending a message. Its output goes to the agent with your next message.

1. Type ! at the very start. The composer shows a Shell tag and Send becomes Run.
2. Type the command, such as pnpm test, and press ↩.
3. The command runs in the agent's shared session, so cd and export carry over to the next one.
4. Press ⌥↩ instead to run it in a fresh shell of its own.

Press ↑ and ↓ in shell mode to reach commands you ran before. Backspace at the start steps back one ! at a time. The folder chip lets you Pick the folder the session runs in.

## Keep a command's output private with !!
<!-- id: terminals/private -->

Type !! at the start of the composer to run a command the agent doesn't see. The tag reads Private, with "The agent won't see the output".

1. Type !! and the command, then press ↩.
2. The result starts folded and stays out of your next message.
3. To share it later, click Attach to next message on a single run, or switch a session block from Private to Shared.

Use it for commands that print secrets or long output the agent doesn't need.

## Work with the shared session
<!-- id: terminals/session -->

The ! commands you run between two of your messages gather in one session card, "Commands you ran on" the device, marked "in one shell". Each is a block with its output and exit code.

- Each block switches between Shared and Private. A line counts what goes to the agent with your next message.
- Send to agent sends the shared blocks now, on their own.
- Add to next message puts them in the next message you send.
- Open in drawer shows the session's tab in the terminal drawer.
- End session closes the shell. While a command still runs, it asks first.

Long output folds to its last 20 lines, with Show all lines. A message takes only the last lines of each block. After a cd, the card shows the folder it's now in.

## Why a command ran on its own
<!-- id: terminals/fresh-shell -->

Some ! commands run in a fresh shell instead of the shared session. The result says why in one line.

- "Sessions need Shell integration, which is off on" the device. Turn it on in that device's Terminals card.
- "Your shell didn't report its commands": the shell gave MaxHQ no marks.
- "The session was busy": a command still ran there.
- "This spans more than one line" or "This starts with exec": the session takes neither.

A command you run with ⌥↩ runs on its own without a note.

## Work with a command that ran on its own
<!-- id: terminals/you-ran -->

A command that ran on its own shows as a "You ran" card in the conversation.

- While it runs it shows the live terminal and a line to type into it, with Stop, then Kill if it keeps going.
- Move to terminal hands it to a drawer tab and leaves a line that links to it.
- A command that asks for a password shows "Waiting for your password", with a secure field. What you type goes only to the terminal.

Once it ends:

- Show output and Hide output fold it.
- Ask about this puts "Look at this output." in the composer.
- Run again, Copy command, and Keep private or Attach to next message.

## Run a code block from an agent's reply
<!-- id: terminals/run-code-block -->

A shell code block in an agent's reply, such as one marked bash, zsh, sh or fish, has a Run menu. Nothing runs from the menu itself, so you can read the command first.

- Run with ! puts the command in the composer in shell mode, ready to edit. Press ↩ to run it, and its output goes to the agent.
- Run privately with !! does the same in private mode.
- Paste into terminal types it into the drawer without pressing Enter.

The menu shows only on replies from agents that take messages, on devices with terminals.

## Turn shell integration on or off
<!-- id: terminals/shell-integration -->
<!-- covers: settings/device/terminals -->

Shell integration lets MaxHQ tell your commands apart in zsh, bash and fish. It's on by default, in the Terminals card on each device's tab in Settings.

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
