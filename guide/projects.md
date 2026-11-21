<!--
SPDX-License-Identifier: MIT
Copyright (c) 2026 Max Boksem
-->

# Projects and workspaces

How MaxHQ groups agents by project, how to add a project and change its settings, how workspaces run agents in containers on a Linux device, and where a project's pull requests show.

## Add a project
<!-- id: projects/add -->
<!-- covers: view/projects -->

A project is a repository. MaxHQ adds one the first time an agent works in a folder of it, and checkouts of the same repository on other devices join that project.

1. Press `⌘N`, or click New agent in the sidebar.
2. In Project, choose Use a local folder. It works on any connected device.
3. Pick the Device, then type the path in Folder, such as ~/code/example-app, or click Browse….
4. Click Start agent, or press `⌘⏎`.

From then on the project shows under Projects in the sidebar. An agent you start yourself in a terminal shows up too, in the project of the folder it runs in.

How MaxHQ tells projects apart:

- A git checkout belongs to the project of its origin remote, or its first remote when it has no origin.
- The SSH and HTTPS addresses of one repository count as the same, so git@github.com:owner/repo.git and https://github.com/owner/repo share a project, on every device.
- The project is named after the last part of the remote's address, in lower case, such as repo.
- A checkout with no remote, or whose remote is a folder on disk, is a project of its own on that device, named after its folder.
- A folder outside any git checkout is a project of its own too.

A project can't be renamed.

## Find a project in the sidebar
<!-- id: projects/sidebar -->
<!-- covers: view/projects -->

The switch at the top of the sidebar reads Projects and Devices. Projects lists each project with its active agents, and older idle agents fold into an "older" row.

- The + on a project's row opens New agent in that project.
- ⋯ (More), or a right-click on the project, opens its menu: New note, New agent, Pull requests, Copy to another device…, Hide project and Project settings….
- Hide project takes the project out of the sidebar. It still shows while one of its agents works, needs you, has news or hit an error.
- To bring a hidden project back, open Hidden projects under the groups and click Show project.

## Follow a project's workflow runs
<!-- id: projects/workflows -->
<!-- covers: view/projects -->

A workflow runs work in steps, such as a build pipeline or an agent that starts other agents. When a run writes its progress the way MaxHQ reads it, MaxHQ draws it under its project. MaxHQ never runs a workflow itself.

- Under a project in the sidebar, a Workflows row counts its runs and folds open to list them. Each run shows how far it got, such as 11/18, or how long ago it ended. A project with no runs has no such row.
- An agent running a workflow carries a chip on its sidebar row and in its header, such as "pipeline 11/18".
- In the agent's view, the Sub-agents and Workflow tabs slide out from the side. Open canvas puts the run in a pane of its own, where Show the run as picks Graph or Timeline, and Show possible loops marks steps that can repeat.
- Right-click the Workflows row for Collapse all and Expand all. Right-click a run for Stop run… and Copy run ID.

A workflow describes itself in a file under .maxhq/workflows in the project, or in a folder directly under it, such as a tool cloned into the project. Without one, MaxHQ shows an agent and its sub-agents as a run of its own.

maxd follows the runs in agents' folders and in open projects. It checks them every second and looks for new ones every 30 seconds. A run quiet for 6 hours rests, and is then checked every 30 seconds only. On a phone, a project's menu has Workflows, with each run's steps.

## Open a project's settings
<!-- id: projects/settings -->
<!-- covers: settings/project -->

A project's settings open from its menu in the sidebar: click ⋯ on the project, then Project settings…. They also show under Projects in Settings.

- General holds Workspaces, the project's containers on devices with Incus, with New workspace and Copy to another device….
- Below that, each device that hosts workspaces for the project gets an "On {device}" part, kept on the device. Change a field, then click Save.
- Skills holds the project's own skills.

A device keeps a project's settings once it hosts workspaces. A device whose maxd is too old says to update it first.

## Settings a project keeps on each device
<!-- id: projects/device-settings -->
<!-- covers: settings/project -->

Each "On {device}" part of a project's General tab changes how that device treats the project's agents and containers.

- Resume after a restart: when the device or a container restarts, busy agents carry on with a message naming what got cut off. Off leaves them paused.
- Ports never offered: ports MaxHQ never offers to forward, such as 9229, 35729. See [Ports and forwards](ports.md).
- Auto-settle after: days with no activity before an agent here settles. Empty uses the device's value, and 0 never settles.
- Settle an agent when its PR merges: off leaves it in the inbox.
- Give agents the workflow's variables: Claude agents can get the variables the project's workflow names, once you allow it on the card the agent shows.
- Memory per container: a soft limit in GiB. Empty uses the host's.
- Commit author: the Name and Email on commits agents make in the project's containers. GitHub still shows the App as the pusher.

## Review the commands agents may run
<!-- id: projects/allowed-commands -->
<!-- covers: settings/project -->

Allowed commands lists the commands you answered "Always allow" for. Agents in the project run them on that device without asking, except in Plan mode.

1. Open the project's settings and find Allowed commands under "On {device}".
2. Read each rule. A rule covers a command and its subcommand, so "git show" covers any git show. One marked Exact covers only that line.
3. Click Copy command to copy a rule, or Remove to drop it. The agent asks again the next time it runs that command.

The list fills from your answers on approval cards. It reads "None yet." until you answer one with Always allow.

## Make a workspace container
<!-- id: projects/new-workspace -->

A workspace is an Incus container on a Linux device that holds a clone of the project. Agents you start in it run inside the container.

1. Open the project's settings and click New workspace under Workspaces. A device's page has the same button under "Workspaces on {device}".
2. Pick the Project and the Device. Only devices with Incus can host one.
3. Change the Container name if you like. Use lower-case letters, digits and hyphens.
4. Tick Start at boot to start the container whenever the device boots.
5. Click Create.

The steps show as they run. It keeps going on the device if you close this. When it says the workspace is ready, pick it under Runs on in the New agent form.

## Start, stop or delete a workspace
<!-- id: projects/workspace-card -->

Each workspace has a card under Workspaces in the project's settings and on its device's page, with its state, guest, egress check, unsaved work and path.

- Start and Stop run the container. While the device is offline, the card says to connect first.
- Delete removes the container. When it holds work that isn't pushed, the card names it and asks for Discard and delete. Keep it backs out.
- Snapshots lists its snapshots. Restore as new makes another workspace from one and changes nothing that's there. Roll back returns the container to one, and over unsaved work it says what you'd lose and asks for Discard and roll back.
- The card's More menu opens the container in Cursor or VS Code over SSH. The first time, MaxHQ asks to add one line to your SSH config: choose Let MaxHQ add the line or I'll add it myself.

## Choose the network and storage pool for workspaces
<!-- id: projects/pools-networks -->
<!-- covers: settings/device/workspaces -->

The Incus network and storage pool new workspaces use are set per device, in the New workspaces card on that device's tab in Settings.

1. Switch the sidebar to Devices and click the Linux device.
2. Find the New workspaces card.
3. Pick the Network new workspaces join. Pick one with a guard that keeps containers off your network.
4. Pick the Storage pool they live on.

The card shows only on a device with Incus. Workspaces you already made keep their network and pool. See [Devices and links](devices.md).

## Copy a project to another device
<!-- id: projects/copy -->

Copy to another device… clones the project onto a second device, into a container or a folder, and can bring the files git ignores, such as env files.

1. Click ⋯ on the project, then Copy to another device….
2. Pick the Project, From and To devices.
3. Under Copy into, pick a container, then a Container name, or a folder, then the Parent folder and Folder name.
4. Under Ignored files to copy, tick the files git ignores that should come along, or pick None or All. Env files start ticked.
5. Click Copy.

It keeps going if you close this. A copy over 100 MB says it takes a while, and the end lists any files that weren't copied.

## Review a project's pull requests
<!-- id: projects/pulls -->
<!-- covers: view/pulls -->

A project's pull requests open from the Pull requests entry at the top of its group in the sidebar, which counts how many are ready to merge. It shows once a GitHub App reaches the project's repository.

1. Click Pull requests under the project, or choose Pull requests from its ⋯ menu.
2. Pick Open or Closed, and change the order with Sort:.
3. Click a row to read it. The tabs are Summary, Files, Checks and Conversation.
4. On Summary, read the Reviews, then click Approve, or pick a Merge method and click Merge.
5. Click Open this agent to go to the agent linked to it, and Back to return.

Merging needs your GitHub sign-in in Settings. To reply, click Reply on GitHub or Open on GitHub. See [Settings](settings.md).

## Link a pull request to an agent
<!-- id: projects/link-pr -->
<!-- covers: view/pulls -->

An agent's pull requests show as chips in its header and in its sidebar row. Click one to read the PR beside the conversation.

- To link one by hand, open the header's menu and choose Link a pull request, then type the number, such as 142, and click Link.
- A chip's menu has Open on GitHub, a message to the agent about the PR, Unlink and Link another PR.
- In a narrow header the first chip stays and the rest fold into +N.

With Settle an agent when its PR merges on in the project's settings, the agent settles once its linked PR merges.

MaxHQ also links a pull request to an agent by itself, in a repository on GitHub. It does this for agents, not their sub-agents:

- When the agent runs gh pr create in its project's repository and the output gives the new PR's address.
- When an open PR's branch, in the repository itself rather than a fork, is the branch the agent works on. A push alone links nothing until the PR is open.

MaxHQ looks for these while the agent is active, or settled less than 24 hours ago, and reads the repository's open pull requests every 2 minutes. It reads them with the GitHub App where the App reaches the repository, and with the device's own gh sign-in otherwise. A PR you unlink stays unlinked.
