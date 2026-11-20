<!--
SPDX-License-Identifier: MIT
Copyright (c) 2026 Max Boksem
-->

# Ports and forwards

How MaxHQ notices ports an agent's processes listen on, how to forward one from another device to this Mac, what the agent still runs, and how to keep a port from being offered.

## Forward a port an agent opened
<!-- id: ports/forward -->

When a process an agent started listens on a port on another device, a card above the composer offers to forward it to the same port on this Mac.

1. The card reads, for example, "pnpm dev on devbox is listening on port 5173."
2. Click Forward. The card then reads "Forwarded to localhost:5173".
3. Click Open in browser to open it.
4. Click Stop forwarding when you're done. The offer doesn't come back, and the port stays listed in the agent's header.

If the port is busy on this Mac, MaxHQ picks another, and the card says "5173 is busy on this Mac, forwarded to" that one.

## Open a port on this Mac
<!-- id: ports/local -->

A port on this Mac needs no forward. Its card offers Open localhost: and the port instead, such as Open localhost:3000, which opens it in your browser.

Click Dismiss to hide a card you don't need. The port stays listed in the agent's header, where you can open or forward it later.

## When a forward stops or waits
<!-- id: ports/forward-states -->

A forward lasts while the port listens and the device can be reached. The card says what changed.

- When the process stops listening, the forward stops too, and a quiet line says "Port 5173 closed on devbox. Forwarding to localhost:5173 stopped."
- When the device can't be reached, the card says "devbox can't be reached. The forward picks up again once it's back." It offers no new forwards meanwhile.
- A device that doesn't forward ports gets no offer. The header lists its ports with no Forward button.

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
