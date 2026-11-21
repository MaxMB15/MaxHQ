<!--
SPDX-License-Identifier: MIT
Copyright (c) 2026 Max Boksem
-->

# Ports and forwards

How MaxHQ notices ports an agent's processes listen on, how to forward one from another device to this Mac, what the agent still runs, and how to keep a port from being offered or turn the offers off.

## Forward a port an agent opened
<!-- id: ports/forward -->

When a process an agent started listens on a port on another device, a card at the top of the agent's view offers to forward it to the same port on this Mac.

1. The card reads, for example, "example-app on devbox is listening on port 5173." and asks to forward it to localhost:5173.
2. Click Forward. The port leaves the card and moves to the place button in the agent's header, which counts the forwarded ports.
3. Click the place button, then the open button on the port's row to open it in your browser.
4. Click the stop button on the same row when you're done. The offer doesn't come back, and the port stays in the list.

If the port is busy on this Mac, MaxHQ picks another, and the row says "5173 is busy on this Mac, forwarded to" that one.

## Open a port on this Mac
<!-- id: ports/local -->

A port on this Mac needs no forward. Its card offers Open localhost: and the port instead, such as Open localhost:3000, which opens it in your browser.

Click Dismiss to hide a card you don't need. The port stays listed in the agent's header, where you can open or forward it later.

## Work through a stack of ports
<!-- id: ports/stack -->

When an agent's processes open several ports, the card shows the newest one, and the rest stack under it in one line.

- The line counts them, such as "6 more ports", and names the first four by number, with +2 for the rest.
- Forward all forwards every port in the stack, the card's too. It skips a port that's already on its way. Ports on this Mac get no Forward all, since a port that listens isn't always a web page.
- Dismiss all hides every card in the stack.
- Show all, or a port's number, opens the list of them, each with its own buttons. Hide closes it.

Forwarded and dismissed ports leave the stack. The header still lists them all.

## When a forward stops or waits
<!-- id: ports/forward-states -->

A forward lasts while the port listens and the device can be reached. The agent's view says what changed.

- When the process stops listening, the forward stops too, and a quiet line says "Port 5173 closed on devbox. Forwarding to localhost:5173 stopped." Only the latest one shows. Its X clears it.
- While the device can't be reached, the agent's view offers no ports, since a forward could only fail.
- A device that doesn't forward ports gets no offer. The header lists its ports with no Forward button.

A forward stays through a lost connection: once the device is back, it asks for the port again. A server that restarts within 10 seconds keeps its forward too, since maxd keeps the port listed that long.

Forwards end when MaxHQ quits, and none come back when you open it again. Forward the port again from the agent's header.

When maxd itself restarts, the port shows as new, so the old forward ends and the card offers the port again.

## Turn off port prompts
<!-- id: ports/prompts -->
<!-- covers: settings/editor -->

Port prompts, in the Conversation section of Settings › Editor, turns the port cards off in every agent's view. It's on by default.

1. Open Settings (⌘,) and pick Editor.
2. Under Conversation, turn off Port prompts.

With it off, the quiet lines about stopped forwards go too. Each agent's header still lists its ports, with Forward and Open there. The setting is kept on this Mac.

## See every port and what the agent runs
<!-- id: ports/popover -->

The place button in an agent's header names its device and container, and counts its forwarded ports. Click it for the list.

- Forwarded ports: each with Open and Stop forwarding.
- Listening, not forwarded: ports the agent's processes opened, each with Forward. On this Mac each has Open.
- Other ports: ports in the same place that no agent opened.
- Running: the processes the agent started.

The list reads the device again every few seconds while it's open. It reads "Nothing listens on a port here yet." when there's nothing to list.

## Stop what an agent left running
<!-- id: ports/running -->

The Running part of the header's popover lists what the agent started and still runs, one row per command with its CPU and processes. The chevron unfolds what runs under it.

1. Click the place button in the agent's header.
2. Find the command under Running and click Stop. Stop reaches everything under it.
3. If it reads "Still running after Stop. Kill ends it at once.", click Kill.

A row marked "after the turn" still runs after the agent's turn ended. One marked "keeps awake" only keeps the computer awake. These stop with the agent. The agent's own CLI and MCP servers sit in a folded group at the bottom.

## Ports MaxHQ never offers
<!-- id: ports/never-offered -->

MaxHQ shows a port once it has listened through two checks in a row, so a port that opens for a moment isn't offered. Some ports it never offers.

- 22 and 5355 on any device, and 53 on systemd-resolved's own addresses.
- MaxHQ's own sockets in a workspace container, and the ports an agent's CLI listens on itself.
- The ports you list under Ports never offered in the project's settings.

To add one, click ⋯ on the project, then Project settings…. Under "On" the device, type the ports in Ports never offered, such as 9229, 35729, and click Save. See [Projects and workspaces](projects.md).
