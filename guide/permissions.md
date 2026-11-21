<!--
SPDX-License-Identifier: MIT
Copyright (c) 2026 Max Boksem
-->

# Permissions

What agents may do without asking you, how to answer when they ask, and how far an agent or an outside tool may go when it runs other agents.

## Pick a permission mode
<!-- id: permissions/modes -->

The permission mode sets what an agent does without asking. Pick it in the composer's footer. A change applies from the agent's next message.

- Ask first: asks before every edit and command.
- Accept edits: edits apply, commands still ask.
- Auto: approves what looks safe and runs the repo's own test, build and lint scripts. Use it on repos you trust.
- Plan only: reads and plans, changes nothing. Codex has no plan mode.
- Full access: never asks. Use it in a workspace container.

New agents on a device start in that device's Default mode, on the New agents card of the device's tab in Settings. Local ACP agents don't offer Auto.

## Use Auto mode
<!-- id: permissions/auto -->

Auto lets an agent run what looks safe without asking: commands that only read, inside the agent's folders, and the repo's own test, build and lint scripts. Everything else still asks.

1. Open the permission picker in the composer's footer and pick Auto.
2. The first time, a dialog explains it. Click Use Auto.

An agent can also be in Auto because its device's Default mode is Auto. Its first start or message then asks the same once. Ask first instead switches the agent and the device's default to Ask first. The app remembers your answer. Forget, under Remembered answers in Settings › Permissions, makes it ask again.

## Answer a permission request
<!-- id: permissions/approval-card -->
<!-- covers: shortcut/pick-the-main-option-on-an-approval-card-with-focus -->

A permission request card shows in the agent's feed when it wants to edit or run something. Click an option, or press Return to pick the main one on the blue button.

- Claude offers Allow once, Allow for this session when its CLI gives a rule, and Deny.
- Codex calls it an approval request, with Approve, Approve for this session, Decline and Decline and stop.
- Cursor and other ACP agents show the options their CLI sends, under the heading Permission request. Cursor sends Allow once, Allow always and Reject. The first allow once option is the main one.

For a command MaxHQ's rules can't cover, the card leaves out Allow always, since the next run would ask again. Allow always on an ACP card also saves MaxHQ's rule for the project.

To say no with a reason, type it in "Why not, or what to do instead" and deny. Claude's turn continues with your reason, and Codex and ACP agents get it as their next message. A session MaxHQ doesn't hold says to answer it in the terminal.

## Answer an agent's question
<!-- id: permissions/questions -->
<!-- covers: shortcut/pick-that-option-on-a-question-card-with-focus -->

A question card shows when an agent asks you to choose. Click an option, or press its number with the card in focus, then send.

1. Read the question and its options.
2. Press 1 to 9 to pick, or click one. Type in Something else for an answer of your own.
3. Press Return, or click Send answer.

Dismiss closes the card and stops the agent's turn, so use it only when you don't want to answer at all.

## Allow a command for good
<!-- id: permissions/always-allow -->

When a command card offers Always allow, picking it saves a rule for that project, so agents run the command without asking in every mode but Plan only.

A rule covers the command and its subcommand, so "git show" covers any git show. A command that deletes, moves, overwrites, reaches the network, runs other code or publishes keeps its exact words, so only that same line passes again.

To see or remove rules:

1. Open the project's ⋯ menu, or right-click it in the sidebar, and pick Project settings….
2. On the General tab, find Allowed commands for the device.
3. Click Remove on a rule. Rules marked Exact cover only that line.

## Approve an agent in a container reaching out
<!-- id: permissions/consent -->

A consent request card shows when an agent in a container wants to reach outside it, such as starting or messaging an agent outside the container. It says what the agent wants and what could go wrong.

1. Read the card's risk and details. Values that look secret stay hidden until you click Show.
2. Click Allow to let this one call go, or Deny to refuse it.
3. Tick Remember for this permission to keep the answer.

A card nobody answers in time refuses the call. To change a remembered answer, open the agent header's ⋯ menu, pick Change what it can run and set Containers.

## Let an agent run other agents
<!-- id: permissions/allow-agent -->
<!-- covers: settings/grants settings/new-permissions -->

An agent can start, message and stop other agents once you allow it. Open its header's ⋯ menu and pick Allow to run agents.

Reading other agents needs no permission while the Other agents card in Settings › Permissions allows it for the device, as it does by default. See [Devices and links](devices.md).

1. In the sheet, tick the Devices, Vendors, Accounts and Places it may use. Any account and Anywhere leave them open.
2. Under Test hosts, tick the kinds it may lease: iOS Simulator, Android emulator, Linux or macOS VM.
3. Set Containers to Ask, Always allow or Never.
4. Set Depth and the limits: Agents started, Agents at once, Usage cap and Expires after.
5. Click Allow.

The agent then shows under Agents allowed to run agents, in the Grants card in Settings › Permissions, where Revoke takes the permission back.

## Set the default limits for agents that run agents
<!-- id: permissions/defaults -->
<!-- covers: settings/new-permissions -->

New permissions, in Settings › Permissions, sets the depth and limits a new permission starts with. By default it's 3 levels deep with no limits.

1. Open Settings and pick Permissions.
2. Under New permissions, set Depth: how many levels of agents may sit below the one that holds the permission.
3. Fill in Agents started, Agents at once and Usage cap. Leave one empty for no limit.
4. Click Save defaults.

Usage cap is a percent from 1 to 100 of the account's limit. MaxHQ compares it with the account's most used window, such as the week, and refuses only new starts past it: "That account has used more than 80 percent of its limit. Pick another account, or wait for it to reset." Agents already running carry on. An account MaxHQ reads no usage for, such as Cursor, is never stopped by the cap.

Limits count agents and usage. They don't fence off files, so keep an agent away from your files by running it in a container.

## Give an outside tool an access key
<!-- id: permissions/access-keys -->
<!-- covers: settings/access-keys -->

Access keys let tools outside MaxHQ, such as a bot, run agents within limits you set. Make one under Access keys in Settings › Permissions.

1. Under New access key, type a Key name, such as slack-bot.
2. In Kept on, pick the device that keeps the key.
3. Set what it may use, the same way as for an agent, then click Make key.
4. On the key's row, click Copy MCP command and add it to the tool, or Copy path for the key's file.

Revoke on a key's row stops every tool that uses it.
