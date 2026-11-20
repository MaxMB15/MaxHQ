<!--
SPDX-License-Identifier: MIT
Copyright (c) 2026 Max Boksem
-->

# Getting started

What MaxHQ does, a tour of its window, and how to start your first agent.

## What MaxHQ does
<!-- id: getting-started/what-maxhq-does -->

MaxHQ starts and watches coding agents, such as Claude Code, Codex and Cursor, on your Mac and your other devices, all from one window.

Each device runs a small service called maxd. It starts the agents, keeps their conversations and answers the app. The Mac app shows the agents of every device together, grouped by project, so an agent on devbox sits next to one on your Mac when both work on owner/repo.

An agent you start yourself in a terminal shows up too. MaxHQ follows it there, and you can take it over later; see [Agents and the composer](agents-composer.md).

## Find your way around the window
<!-- id: getting-started/window -->
<!-- covers: shortcut/show-or-hide-the-sidebar shortcut/resize-the-sidebar-on-its-edge-with-focus -->

The sidebar on the left lists your agents, the main area shows the ones you open, and the title bar holds the dispatcher, usage and Settings.

From left to right, the title bar has:

1. The sidebar button, which hides or shows the sidebar (⌘B).
2. The sun button, **Keep the screen on**, which stops the screen dimming and the Mac sleeping, on battery too.
3. The **Ask the dispatcher** box. See [The dispatcher](dispatcher.md).
4. The usage meter, with **Plan usage** for each account, the **Fullest context windows** and **Check usage now**.
5. The bell, which counts the agents that need you. Click it to open the next one (⌘J).
6. The gear, which opens Settings (⌘,).

To make the sidebar wider or narrower, drag its edge, or give the edge focus and press the Left and Right arrows.

## See your agents by project
<!-- id: getting-started/projects -->
<!-- covers: view/projects shortcut/filter-the-agents shortcut/open-the-first-agent-the-filter-shows shortcut/clear-the-filter-then-leave-it shortcut/move-between-the-agents-on-an-agent-with-focus shortcut/open-the-agent-on-an-agent-with-focus -->

Pick **Projects** at the top of the sidebar to list your agents by project. A project is one repository, wherever it's checked out.

- **Needs you** at the top lists the agents waiting on an answer from you, the longest waiting first.
- Under each project sit its active agents. Idle agents over a day old fold into an **older** row.
- A project also shows **Pull requests** when a GitHub App reaches it, and **Notes** when it has notes.
- **Hidden projects** at the bottom holds the projects you hid.

Type in **Filter** (⌥⌘F) to narrow the list. Enter opens the first agent it shows, and Esc clears the filter, then leaves it. With an agent in focus, the Up and Down arrows move between agents and Enter opens one.

## How projects show up
<!-- id: getting-started/how-projects-appear -->

MaxHQ builds the project list from where your agents work, so you never add a project by hand.

- Checkouts of the same remote share one project, on every device. An agent in ~/code/example-app on your Mac and one in the same repository on devbox both sit under it.
- A checkout with no remote is a project of its own, named by its device and folder.
- An agent started in a terminal shows up under its project as soon as it runs.

To start an agent in a folder MaxHQ doesn't know yet, pick **Use a local folder** in the New agent form.

Right-click a project, or click its **⋯** button, for **New agent**, **Pull requests**, **New note**, **Copy to another device…**, **Hide project** and **Project settings…**.

## See your devices
<!-- id: getting-started/devices -->
<!-- covers: view/devices -->

Pick **Devices** at the top of the sidebar to list your agents by where they run, with each device's connection.

The list starts with this Mac, then each network, then each device, with its container workspaces under it. Click a device to open its page.

At the foot of the list:

- **Add device** connects another computer or server over ssh, such as devbox.
- **Refresh all** checks every device again.
- The update capsule shows this app's version, and **Update everything** when updates wait.

## Start maxd on this Mac
<!-- id: getting-started/start-maxd -->

MaxHQ needs maxd running on your Mac. When the app can't reach it, a notice offers **Start maxd**.

1. Click **Start maxd** in the notice.
2. Wait for your agents to show up in the sidebar.

This Mac's page under **Devices** in Settings can also start maxd, or uninstall it.

## Open Settings
<!-- id: getting-started/settings -->
<!-- covers: view/settings shortcut/open-settings shortcut/show-keyboard-shortcuts -->

Click the gear at the right of the title bar, or press ⌘,, to open Settings.

Type in **Search settings** to find a setting by name. Matching devices follow the app's pages, and picking one opens that device's page at the matching card.

The list has four groups:

- **App**: settings for this Mac, such as **Appearance**, **Sidebar**, **Editor**, **Dispatcher**, **Agent tools**, **Notifications** and **Keyboard shortcuts**.
- **Logins**: your Claude, Codex, Cursor and GitHub sign-ins.
- **Projects**: each project's defaults and skills.
- **Devices**: each device's own settings, such as the mode new agents start in.

Press ⌘/ to see every keyboard shortcut. See [Settings](settings.md) for each page.

## Start your first agent
<!-- id: getting-started/first-agent -->

Press ⌘N, or click **New agent** at the foot of the sidebar, to start an agent.

1. Pick the **Project**, and where it runs.
2. Pick the **Agent**, which is the account it uses, and a **Mode**.
3. Type a **First message**, such as "Add a dark mode toggle to example-app".
4. Click **Start agent**, or press ⌘Enter.

The agent's pane opens once it's up. When it asks something, it moves to **Needs you** and the bell counts it. See [Agents and the composer](agents-composer.md) for the rest.
