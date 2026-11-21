<!--
SPDX-License-Identifier: MIT
Copyright (c) 2026 Max Boksem
-->

# Troubleshooting

What to do when a device won't connect, a sign-in runs out, an agent waits on a usage limit or seems stuck, what the agent states mean, and where maxd's log is.

## Fix a device that won't connect
<!-- id: troubleshooting/device-wont-connect -->

A device's page shows where the connection stops. Open it from the Devices view of the sidebar, read the first check that failed, and click Try now.

1. Switch the sidebar from Projects to Devices and click the device.
2. Read the connection card. "Can't reach" names the reason and when it last answered.
3. Under "Where the connection stops", find the first failed check: Net, Tailnet, Peer, Ping, SSH, maxd or Agents. Each check runs only if the one above passed.
4. Fix that step, then click Try now to run every check again from the top.

For an SSH route, Copy SSH command lets you try the same login in a terminal. Tailscale is optional, so the Tailnet checks only matter for devices you reach through it. MaxHQ keeps trying in the background meanwhile.

## Read the reconnect banner
<!-- id: troubleshooting/banner -->

A banner at the top says when MaxHQ lost a device. It reconnects on its own, and what you see until then is what the device last sent.

- "You are offline": this Mac has no network. MaxHQ reconnects once you're back.
- "Reconnecting to" a device: the link dropped and MaxHQ is setting it up again.
- "Can't reach" a device, or your devices: MaxHQ keeps trying in the background.

The banner also says when the device last answered. Open the device's page to see which check fails.

## Let a new app connect to a device
<!-- id: troubleshooting/waiting-approval -->

"Waiting for approval on devbox" means devbox doesn't know this app's key yet. Allow it from MaxHQ on a Mac that already connects to devbox.

1. On that Mac, open MaxHQ. The sidebar lists each program asking to get in, such as "Let this program control devbox?". Click it to open the device's page.
2. Read the card: what the device knows about the program, such as how it reached maxd and its key.
3. Click Trust its key. As the card says, "A yes adds the key to App keys in devbox's settings, until you remove it." Don't allow, the first button, keeps it out.
4. Back on the new Mac, the device connects by itself.

- The card shows once a program has waited 10 seconds without proving a key the device trusts. One nobody answers in 10 minutes counts as Don't allow.
- A program with no app key gets Allow until it disconnects instead. Then nothing is kept, and it asks again next time.
- A Mac with no MaxHQ app connected shows the card as a macOS alert from maxd, with Don't allow as its default button.
- A phone shows the card too, but says "Answer this on the Mac".

## Start maxd on this Mac
<!-- id: troubleshooting/mac-maxd -->

Agents on this Mac need maxd running. When MaxHQ can't reach it, the Mac's page says why and offers Start maxd.

1. Open this Mac's page from the Devices view of the sidebar.
2. Read "Can't reach maxd on this Mac" and its reason.
3. Click Start maxd.

If maxd was uninstalled, Start maxd puts it back. To make a running maxd start fresh, click Restart maxd on the device's page. Wind down for restart first asks busy agents to finish the step they're on, so the restart doesn't cut them off.

## Sign in again
<!-- id: troubleshooting/sign-in-again -->

When a CLI's login runs out, the agent's feed shows an error with a Sign in again button. Click it to run the login on the device that holds it.

1. Click Sign in again on the error in the feed.
2. Finish the login in the window that opens.
3. Send the agent a message to carry on.

You can also sign in from Settings, under Logins: pick Claude in workspaces, Codex in workspaces or Cursor in workspaces. A login marked Signed out shows Sign in instead.

## Wait out a usage limit
<!-- id: troubleshooting/usage-limit -->
<!-- covers: settings/device/limits -->

An agent at its account's limit shows "Waiting for usage limit reset". Nothing is lost. It continues on its own at the reset, or waits for your message.

- Continue after the limit decides which. Set it per agent from its right-click menu in the sidebar.
- Each device's Usage limits card sets the default per CLI, for Claude Code and Codex agents.
- Move to another account with room left continues the agent there now. Don't resume stops it from continuing on its own.

The Limit alert in Settings › Notifications tells you when an agent starts waiting (see [Notifications](notifications.md)).

## Wake an agent that's asleep
<!-- id: troubleshooting/asleep -->
<!-- covers: settings/device/idle -->

Asleep means the device stopped the agent's CLI after it sat idle. Your next message starts it again, with nothing lost.

To change how long a CLI may sit idle, open the device's page from the Devices view of the sidebar and set Stop idle CLIs after on its Idle agents card.

Other states on the agent's dot:

- Live: the agent's CLI runs.
- Agent not running: the device answers, but the agent doesn't run. The reason follows.
- Not reachable: MaxHQ can't reach the device. See the device's page.
- Agent state unknown: the device answers, but maxd can't tell yet whether the agent runs.

## Unstick an agent
<!-- id: troubleshooting/stuck-agent -->
<!-- covers: shortcut/stop-the-agent-s-turn -->

When an agent hangs in a turn, stop the turn and send it a message. Press ⌘. or click Stop in the composer.

1. Check the feed for a permission or question card first. The agent may be waiting on you.
2. Press ⌘. to stop the turn.
3. Send a message telling it what to do next.

A card in a session MaxHQ doesn't hold says to answer it in the terminal, since MaxHQ can't answer it. If the agent still won't move, the header's ⋯ menu offers Move to… for another device or account, and Delete agent.

## Fix "needs a newer maxd"
<!-- id: troubleshooting/needs-newer-maxd -->

A feature that says its device's maxd is too old works once that maxd is updated. Update it from the sidebar.

1. Click the update summary at the bottom of the sidebar.
2. Click Update everything.

A device's page also says when its maxd is older than the app, or newer, in which case update the app instead. See [Updates](updates.md).

## Find maxd's log
<!-- id: troubleshooting/logs -->

maxd writes what it does to a log on each device. On a Mac it's the file ~/Library/Logs/MaxHQ/maxd.log. On Linux, read it with `journalctl --user -u maxd.service`.

- When maxd is installed on a Mac, it says "maxd runs now and at every login, and logs to" that file.
- When maxd starts and the file is over 16 MiB, it moves it to maxd.log.1, over the one before, and starts a fresh one. On Linux the journal trims itself.
- RUST_LOG sets how much maxd logs when you run `maxd run` yourself, such as RUST_LOG=debug.
- MaxHQ has no log viewer. Open the file in Console or a text editor.
- The MaxHQ app keeps no log file of its own.
- `maxd service uninstall --delete-data` removes the logs along with maxd's data.
