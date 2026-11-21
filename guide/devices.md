<!--
SPDX-License-Identifier: MIT
Copyright (c) 2026 Max Boksem
-->

# Devices and links

How to add a Mac or Linux machine to MaxHQ, what maxd does on each one, how devices link to each other, and what each card on a device's page changes.

## Add a device over SSH
<!-- id: devices/add -->
<!-- covers: view/devices -->

MaxHQ adds a device with your own SSH login, installs maxd there, and then reaches it through a key of its own.

1. Switch the sidebar to Devices and click Add device.
2. Pick a host From your SSH config, or On your tailnet when you use Tailscale. A host that reads Set up already is in MaxHQ. For any other one, type it under Or another host, as user@host the way ssh takes it, and click Next.
3. Under Logins for the host, you can paste a Claude token from claude setup-token. That turns setup-tokens on for the device, and with more than one Claude account you pick which one it is under Claude account. Leave it empty to sign in with claude on the device later.
4. Click Set up and the host's name, such as Set up devbox.
5. If ssh doesn't know the host yet, the dialog shows its host key. Check it matches what ssh-keygen -lf prints on the host, then click Trust this key, or Don't trust. Nothing runs on the host until you trust its key, and no answer in 10 minutes counts as Don't trust.

The dialog then lists each step as it runs: Reading devbox's host key, Checking the system, Copying maxd, Added MaxHQ's key, Connecting to maxd, and Sending the logins you picked when you gave a token.

On a Linux host where maxd would stop when you log out, the dialog says lingering is off and shows sudo loginctl enable-linger with your user name, with Copy command. Run it on the host to keep maxd running.

When it's done, the dialog reads devbox is set up. Click Show devbox to open its page, or Done. Its agents and settings show under Devices. If a step fails, the dialog says why, and Try again goes back to Logins for the host so you can start it again. Since the token field empties once setup starts, paste the token again if you gave one.

## What a device's page shows
<!-- id: devices/page -->
<!-- covers: view/device -->

Click a device under Devices to open its page. It reads from the top:

- The header, with how MaxHQ reaches it, Try now and Copy SSH command.
- Where the connection stops: the connection checks.
- Load: CPU over the last hour and the busiest processes.
- Setup: what the last host check found, with a fix for each item.
- Workspaces on the device, with New workspace and Copy to another device….
- Moves into MaxHQ, for older containers MaxHQ didn't make.
- Agent CLIs, Tools and ACP registry: what's installed, with versions and updates.
- Test hosts and Leases: the device's test machines, and the agents that hold one now.
- Routes.
- The device's settings cards, from New agents to Network access, then ACP agents and App keys.
- Device links, then Phones with the device's address for a phone.
- Identity, then Restart. This Mac's page also has Start maxd and Uninstall maxd here.
- Agents here, or Agents last seen here while it's offline, settled ones included.

A section that needs the device says "Connect to" it while it's offline.

## Find where a connection stops
<!-- id: devices/connection-checks -->
<!-- covers: view/device -->

The connection checks on a device's page, under Where the connection stops, show where the connection stops. Each check runs only if the one above passed.

1. Open the device's page from Devices in the sidebar.
2. Read the checks from the top. Each one reads Passed, Failed or Not run.
3. Fix the first one that failed, then click Try now to run every check again from the top.
4. To try the same connection in Terminal, click Copy SSH command.

A device you reach only through Tailscale needs Tailscale on this Mac. The sidebar marks those devices. Add a route under Routes to reach one without it.

## Fix a device's setup
<!-- id: devices/setup -->
<!-- covers: view/device -->

The Setup section of a device's page lists what its last host check found, item by item, with the fix for each.

1. Open the device's page and find Setup. It reads All ok, or how many items need a fix.
2. Click Apply for the fixes maxd can make without root.
3. The part that needs root is one command under Root command. Click Copy command, or Open in Terminal and type your sudo password there.
4. Click Check again once it's done.

On NixOS the section gives the change to make to your configuration instead, with Copy change.

## Install tools on a device
<!-- id: devices/tools -->
<!-- covers: view/device -->

Agent CLIs and Tools on a device's page list what MaxHQ installs and keeps up to date there.

- Agent CLIs lists Claude, Codex and Cursor with their versions. See [Updates](updates.md) to update one.
- Tools lists the helpers agents use: the GitHub CLI, Node.js, uv, agent-device, Java and, on a Mac, the Android SDK. Agents installed from the ACP registry show here too.
- Each row reads its version, or Not installed with Install. Remove takes out one MaxHQ installed and asks first. One you installed yourself reads "You installed it." and has no Remove.
- An install shows how far it got, such as Downloading with its size. One that failed says why, with Try again. One that waits on a licence reads Waiting for you to accept its licences, with Read licences.
- Tools in a container workspace sit under In and the workspace's name.

A device whose maxd is too old to install tools says so. Update it first.

## Install an agent from the ACP registry
<!-- id: devices/acp-registry -->
<!-- covers: view/device -->

The ACP registry section on a device's page lists the agents the ACP registry offers, under Agents to install. maxd fetches the list at most every five minutes and keeps a copy for when it can't.

1. Open the device's page and find ACP registry.
2. Type in Search the registry to find an agent by name or what it does. Installed ones come first.
3. Each row says how it installs, as a Binary, an npm package or a Python package, with its version. Click Install, or Update when a newer version is out.

When the registry gives no checksum for a binary, MaxHQ asks before it downloads it, with Install anyway. An installed agent becomes an ACP agent on the device and starts like any other. You sign in to it the way it asks.

When maxd can't reach the registry, the section says how old its copy is.

## Test hosts and leases on a device
<!-- id: devices/test-hosts -->
<!-- covers: view/device -->

The Test hosts section on a device's page shows the test machines it can run for agents there: iOS Simulator, Android emulator, Linux and macOS VM.

- Each kind the device hosts shows where it stands, such as a test container that's ready for a lease, with Build, Reset and Check again. Reset ends a lease on it.
- Lease length is how long a lease lasts when the agent doesn't say, 30 minutes by default.
- Longest lease is the most an agent may ask for, 240 minutes by default.
- Ends when idle ends a lease after this long without a command, 10 minutes by default.
- On a Mac, macOS VMs at once sets how many VMs run together. Apple lets a Mac run two.

Leases lists the test machines agents hold now, each with End lease. Its holder loses the machine. Only agents on a device lease its test machines, so for an agent elsewhere to test here, start an agent on this device. To pick which devices host each kind, see [Settings](settings.md).

## Change how MaxHQ reaches a device
<!-- id: devices/routes -->
<!-- covers: view/device -->

Routes lists the SSH hosts MaxHQ tries for a device. MaxHQ tries them from the top and marks the one that worked last on this network.

1. Open the device's page and find Routes.
2. Type an SSH host under Add route, as user@host the way ssh takes it, such as devbox.example.com.
3. Use Up and Down to change the order. The top one reads Tried first.
4. Click Remove to drop one.

Each change applies the next time MaxHQ connects. A route that works on one network, such as a LAN address at home, can sit above a name that works everywhere.

## What maxd is, and restarting it
<!-- id: devices/maxd -->
<!-- covers: view/device -->

maxd is MaxHQ's service on each device. It runs the agents, keeps their history, and serves the app, your phone and the other devices.

- Restart on a device's page has Restart maxd, which restarts it through the device's service manager. It shows Restarting until the device is back.
- For a restart you plan, such as an update, pick a Window and click Wind down for restart first. Busy agents finish their step and start no long command, then pick up after the restart.
- This Mac's page has Start maxd and Uninstall maxd. Uninstalling stops every agent on this Mac and takes maxd out of your login items. Tick Also delete MaxHQ's data on this Mac to remove that too. Start maxd puts it back.

## Link devices so agents reach each other
<!-- id: devices/links -->
<!-- covers: view/device -->

Device links let agents on one device start, message and wait on agents on another, even with the app closed. Notes sync over them too.

Each device you add links to the others by itself. To see or change a device's links:

1. Open the device's page and find Device links.
2. Each other device reads Linked, Waiting for the device, Off, or why it can't link yet.
3. Click Remove to turn that pair off, or Allow to turn it back on. A device that waits hears once it's back.

A link opens for a call, over SSH with the device's own key, and closes again when nothing uses it.

## Use MaxHQ without Tailscale
<!-- id: devices/tailscale -->

Tailscale is optional. Every feature works over plain SSH, and Tailscale only adds shortcuts on top.

- Add device lists hosts From your SSH config, and On your tailnet only when there is one.
- Any host ssh can reach works, such as a name from ~/.ssh/config or user@host on your network.
- A phone reaches devices over SSH, or over the network when a device's Network access is on. See [Phones](phones.md).
- Devices grouped under a Tailscale network in the sidebar need Tailscale on this Mac to reach them. Add a route without it under Routes to reach them another way.

## New agents: mode and effort
<!-- id: devices/new-agents -->
<!-- covers: settings/device/new-agents -->

The New agents card on a device's page sets the mode and effort new agents there start with. By default that's Auto on the Mac and Full access on remote devices.

- Default mode: new agents here start in this mode unless you pick another. Running agents keep theirs.
- Claude Code effort and Codex effort: the Effort level new agents start at, on a slider over the levels the device's logins list. Until you pick one, each agent starts at its model's own level.

A vendor's effort row shows once a login of it on the device lists levels.

## Other agents: reading without a permission
<!-- id: devices/other-agents -->
<!-- covers: settings/device/other-agents -->

The Other agents card sets whether agents on the device may read other agents without asking you first. Reading is on by default.

With Agents can read other agents on, any agent on the device can list your agents, read their conversations and wait for them, without a permission. It can also list where agents start, pull requests, test hosts and notes, and read notes. A conversation may hold code, paths and command output, so turn it off when that matters.

- Off: agents read only the agents you attach or quote. The rest needs Allow to run agents. See [Permissions](permissions.md).
- Starting, messaging or stopping agents always needs your permission.
- An agent in a container still asks before it reads outside the container.
- An agent on a linked device that reads here follows this device's switch.

A device whose maxd is too old for the switch says so on the card. Update it first.

## Inbox: settle idle agents and snooze at a limit
<!-- id: devices/inbox -->
<!-- covers: settings/device/inbox -->

The Inbox card sets when idle agents on the device settle by themselves. They settle after 5 days by default.

- Auto-settle after: agents with no activity for this many days settle on their own, and any new activity brings them back. 0 never settles them.
- Snooze at the limit: agents that stop at a usage limit snooze until the limit resets, then come back to your inbox.

A project can set its own Auto-settle after for its agents on the device, in the project's settings. See [Projects and workspaces](projects.md).

## Usage limits: continue after the limit
<!-- id: devices/limits -->
<!-- covers: settings/device/limits -->

The Usage limits card decides whether agents stopped by a usage limit carry on by themselves. It's off by default.

- Claude Code agents continue: an agent a limit stops mid-turn gets a message to carry on once the limit resets.
- Codex agents continue: the same for Codex.

Both switches let agents continue after the limit. To change it for one agent, use its menu in the sidebar. Cursor agents always wait for you, since Cursor doesn't say when a limit resets.

## Placement: where unplaced agents run
<!-- id: devices/placement -->
<!-- covers: settings/device/placement -->

The Placement card weighs a device when MaxHQ picks where a new agent runs. It's Normal by default.

- New agents you don't place: Prefer or Normal, the only two choices.
- Prefer makes this device count twice in the pick. Normal counts it once.

MaxHQ picks a device only when you don't pick one, the project's default workspace isn't on a ready device, and more than one connected device has a login for the agent's vendor. It weighs each device by its CPU cores, how idle they are and how much memory is free, times its Placement. It skips a device at 95% CPU or more, one with 5% or less memory free, and one whose last reading is more than 15 seconds old.

Set Prefer on a strong server so agents land there first, and leave a laptop on Normal.

## Idle agents and memory
<!-- id: devices/idle-memory -->
<!-- covers: settings/device/idle settings/device/memory -->

Two cards on a device's page keep agents from holding memory they don't use.

- Idle agents, Stop idle CLIs after: maxd stops the CLI of an agent idle this long to free its memory. The agent stays as it is and starts again with your next message. Pick Never, 15 or 30 minutes, or 1, 2, 4 or 8 hours. It's 1 hour by default.
- Memory, Recent items kept in memory per agent: maxd keeps this many of each agent's newest items in memory, and the rest on disk until you scroll back. It's 256 by default.
- History kept per agent: past this size, an agent's oldest items leave its timeline on this device. They stay in the agent's own history. It's 256 MB by default.

## Terminals: shell integration
<!-- id: devices/terminals -->
<!-- covers: settings/device/terminals -->

The Terminals card turns shell integration on or off for the device. It's on by default.

With Shell integration on, MaxHQ adds hooks to zsh, bash and fish so it can tell commands apart. That gives each command its mark in the terminal, and ! commands need it to share one shell. Off starts shells exactly as your startup files say, and each ! command runs in a fresh shell. A change applies to terminals opened after it. See [Terminals and ! commands](terminals.md).

## Claude setup-tokens on a device
<!-- id: devices/setup-tokens -->
<!-- covers: settings/device/setup-tokens -->

The Claude setup-tokens card lets a device keep a Claude setup-token for its agents. It's off by default, and meant for personal use.

- Take setup-tokens: the device stores the setup-token you send it and gives it to Claude Code agents when they start. Turning it on asks first.
- Without it, sign in with claude on the device itself.

Send the token from the Claude page of Settings, or paste one as the Claude token when you add the device. MaxHQ keeps no copy. See [Settings](settings.md).

## Sleep: keep this Mac awake
<!-- id: devices/sleep -->
<!-- covers: settings/device/sleep -->

The Sleep card on a Mac's page decides when maxd keeps the Mac awake. A sleeping Mac stops maxd, so your phone, webhooks and scheduled tasks can't reach it.

Keep this Mac awake:

- On the charger, the default: awake whenever it's plugged in. On battery, only while an agent works.
- Always: awake whenever maxd runs, on battery too.
- While agents work: awake only while an agent on this Mac works.
- Off.

The display still turns off, and closing the lid still sleeps the Mac.

## Cursor usage on a Mac
<!-- id: devices/cursor -->
<!-- covers: settings/device/cursor -->

The Cursor usage card lets one Mac read the Cursor account's usage for MaxHQ's usage bars. It's off by default, and one Mac covers the whole account.

1. Open the Mac's page and find Cursor usage.
2. Turn on Enable Cursor usage.
3. macOS asks once to let maxd read the Cursor CLI's saved login from the Keychain. Allow it.

maxd then asks Cursor for this month's usage through an endpoint Cursor doesn't document, so it may stop working.

## New workspaces: network and storage pool
<!-- id: devices/workspaces -->
<!-- covers: settings/device/workspaces -->

The New workspaces card shows on a device with Incus. It sets what Incus gives each container MaxHQ makes there.

- Network: the Incus network new workspaces join. Pick one with a guard that keeps containers off your network.
- Storage pool: the Incus storage pool new workspaces live on.

Workspaces already made keep theirs. See [Projects and workspaces](projects.md) to make one.

## Skill library: which device holds it
<!-- id: devices/library -->
<!-- covers: settings/device/library -->

The Skill library card shows which device holds the skill library. One device holds it at a time.

1. Open the page of the device that should hold it.
2. Turn on Holds the skill library.

That moves the library and its sources from the device that held it to this one. To move it again, turn it on for another device.

## Network access for phones
<!-- id: devices/network -->
<!-- covers: settings/device/network -->

The Network access card lets phones on the same network or tailnet reach the device directly while it's on. It's off by default, and phones use SSH without it.

- Network access: the switch.
- Port: the port phones reach the device on. It's 7466 by default.
- Addresses for phones: the addresses the device gives phones to dial.
- Each signed-in phone has Revoke. The phone has to pair again to reach it.

See [Phones](phones.md).

## App keys
<!-- id: devices/app-keys -->
<!-- covers: settings/device/app-keys -->

The App keys card lists the apps that may do everything on the device, one row per key.

- Remove on a key's row makes the device stop trusting it.
- On this Mac's page, This Mac's app key shows the key this Mac's app signs with.
- Make a new app key moves every device to a new key for this Mac. It asks first.

## Run ACP agents on a device
<!-- id: devices/acp-agents -->
<!-- covers: view/device -->

ACP agents are agent programs that speak the Agent Client Protocol. Set one up on a device's page to start it like any other agent there.

1. Open the device's page, find ACP agents and click Add agent.
2. Give it a Name, and a Program: a name on PATH, or a full path.
3. Add Arguments one per row. They're passed as typed, with no shell in between.
4. Add Variables with a name and a value. A saved value shows only as set, with Replace and Remove.
5. Turn on Also run in containers to let it run in the device's workspace containers too. The program must be installed in each one, and you sign in to it there.
6. Click Save. MaxHQ starts it once to check it.

Each row has Edit, Remove and Copy command.
