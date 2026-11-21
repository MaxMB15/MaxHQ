<!--
SPDX-License-Identifier: MIT
Copyright (c) 2026 Max Boksem
-->

# Agents and the composer

How to start an agent, talk to it, answer what it asks, and keep track of many at once.

## Start an agent
<!-- id: agents-composer/start -->
<!-- covers: view/agent shortcut/new-agent shortcut/start-the-agent-in-the-new-agent-form settings/device/placement -->

Press ⌘N, or click **New agent** at the foot of the sidebar or in a project's menu. MaxHQ starts the agent and holds it, and its pane opens once it's up.

1. Pick the **Project**, or **Use a local folder** to pick a **Folder** on any connected device.
2. Pick where it runs: **Runs on** for a project's workspace, or **Device** for a folder. A container workspace also takes a branch.
3. Pick the **Agent**, which is the account it uses, then its **Model** and **Mode**.
4. Give it a **Title** and a **First message** if you like. Without a title, the CLI or the first message names it.
5. Click **Start agent**, or press ⌘Enter.

When you leave the place open, MaxHQ picks the device with the most free CPU and memory. A device's **New agents you don't place** setting can make it preferred.

## Pick a mode
<!-- id: agents-composer/modes -->
<!-- covers: settings/device/new-agents -->

The mode says what an agent may do without asking you. Pick it in the New agent form, or later from **Permission mode** in the composer's footer.

- **Ask first** asks before every edit and command.
- **Accept edits** applies edits, and commands still ask.
- **Auto** approves what looks safe and runs the repository's own test, build and lint scripts without asking. Use it on repositories you trust. MaxHQ asks "Use Auto mode?" the first time.
- **Plan only** reads and plans, and changes nothing. Codex doesn't have it.
- **Full access** never asks. Use it in a workspace container.

Each device sets the **Default mode** for new agents in its **New agents** card. It's Auto on the Mac and Full access on remote devices until you change it. Running agents keep theirs.

## Set how hard an agent thinks
<!-- id: agents-composer/effort -->
<!-- covers: settings/device/new-agents -->

For Claude Code and Codex agents whose model has effort levels, the composer's footer shows the current level, such as **High**. Click it for **Effort** and drag the slider to pick a level; **Default** marks the model's own.

The levels come from the CLI, for each model, so not every model has every one. From lowest to highest they read **None**, **Minimal**, **Low**, **Medium**, **High**, **Extra high** and **Max**. A level MaxHQ doesn't know yet shows under the CLI's own name.

A change applies from the next message you send. Only agents MaxHQ holds have it, not sub-agents or sessions started in a terminal.

New agents start at the level their device sets. To change that, open the device's page (pick **Devices** at the top of the sidebar and click the device) and use **Claude Code effort** or **Codex effort** in its **New agents** card.

## Send a message
<!-- id: agents-composer/send -->
<!-- covers: shortcut/send-or-queue-while-the-agent-works shortcut/send-at-once-to-steer-the-agent shortcut/new-line shortcut/bring-back-an-earlier-message-with-the-caret-at-the-start settings/editor -->

Type in the composer at the bottom of an agent's pane and press Enter. Shift-Enter or Option-Enter starts a new line.

While the agent works, Enter does what **While an agent works, Enter** says in Settings, under **Editor**:

- **Queue** waits until the agent finishes, then sends.
- **Steer** sends at once, and the agent reads it between steps.

⌘Enter does the other one.

With the caret at the start, the Up and Down arrows bring back earlier messages.

Under **Typing** on the same page, **Check spelling** underlines misspelled words in the composer and in notes, and **Correct spelling automatically** lets macOS fix words as you type.

## Change or send a queued message
<!-- id: agents-composer/queue -->
<!-- covers: shortcut/edit-the-last-queued-message-from-an-empty-composer -->

A queued message waits above the composer with "Queued, sends when this turn ends", and goes when the agent's turn ends.

- **Steer now** sends it at once while the agent works.
- **Edit** puts it back in the composer. From an empty composer, the Up arrow edits the last one.
- **Remove** drops it.

When the agent waits on an approval or a question, the composer reads "Queue a message for after this turn". **Send now** (⌘Enter) skips what's waiting and sends your message now. **Go to the request** scrolls to the card.

## Add context with pills
<!-- id: agents-composer/pills -->

Type a trigger in the composer to pick something, and it goes in as a pill the agent can read.

- `@` picks files, folders, agents and notes.
- `#` picks pull requests and issues. Pasting a GitHub link to one, such as a pull request on owner/repo, turns it into a pill too.
- `$` picks skills.

To quote part of a conversation, select the text and choose **Add to chat**.

A message that starts with `!` runs as a shell command instead, and `!!` keeps it from the agent. See [Terminals](terminals.md).

## Attach files and pictures
<!-- id: agents-composer/attach -->
<!-- covers: shortcut/attach-files -->

Give an agent files, folders or pictures along with your message.

- Click the paper clip, **Attach files or folders**, or press ⌘U.
- Drop files on the composer.
- Paste a picture.

The attachments show above the text, and go with the next message you send. A queued message keeps its attachments.

## Zoom into a picture
<!-- id: agents-composer/zoom -->

Click a picture in a conversation to see it whole over the window. It zooms and pans as in Preview, and so does a picture open in a file pane.

- Pinch, or scroll with ⌘ held, to zoom around the pointer.
- Scroll or drag to move around a picture bigger than its frame.
- Double click to go between fitted and actual size.
- Press + or - to zoom, 0 to fit it and 1 for actual size.
- The control in the corner has **Zoom out**, **Actual size**, **Zoom in** and **Zoom to fit**.

A picture zooms from 10% to 800%.

## Answer an approval card
<!-- id: agents-composer/approval -->
<!-- covers: shortcut/pick-the-main-option-on-an-approval-card-with-focus -->

When an agent wants to run a command or make an edit its mode doesn't allow, it shows a **Permission request** (Claude Code) or **Approval request** (Codex) card, and waits.

1. Read what it wants to do.
2. Click one of the options the CLI offers. With the card in focus, Enter picks the main one.

To turn it down and say why, type in **Reason for denying** (or **Reason for declining**), next to the button that refuses, then click that button. Claude Code reads the reason and the turn continues. Codex gets it as your next message.

The card then reads **Request allowed** or **Request turned down**. For a session started in a terminal, answer in the terminal; MaxHQ can't answer a session it doesn't hold.

## Answer a question
<!-- id: agents-composer/question -->
<!-- covers: shortcut/pick-that-option-on-a-question-card-with-focus -->

When an agent asks you something, it shows **A question for you** (or several questions) and waits.

1. Pick an option. With the card in focus, the keys 1 to 9 pick that option.
2. Or pick **Something else** and type your own answer.
3. Click **Send answer** (or **Send answers**).

To skip the questions, click **Dismiss**. That stops the turn too.

When Claude Code proposes a plan, it shows as a **Proposed plan** card.

## Stop a turn or an agent
<!-- id: agents-composer/stop -->
<!-- covers: shortcut/stop-the-agent-s-turn -->

To stop what an agent is doing right now, click the stop button in the composer, **Stop this turn**, or press ⌘.. The agent keeps its conversation and waits for your next message.

To end the agent's process, open the **More** menu in its header and pick **Stop agent**. Its pane then reads **Stopped**: "The conversation stays. Resume starts it again, and so does sending it a message." Click **Resume**, or just send a message. Its terminals keep running, and the sidebar marks it **Paused** until it starts again.

The same menu can copy the session ID, **Export as Markdown…** or **Copy as Markdown**, and **Delete agent**.

## Work through what needs you
<!-- id: agents-composer/inbox -->
<!-- covers: shortcut/open-the-next-agent-waiting-on-you shortcut/settle-and-go-to-the-next-agent-waiting-or-settle-the-picked-ones shortcut/pin-or-unpin-the-chosen-agents shortcut/undo-the-last-change-while-its-notice-shows settings/device/inbox -->

The sidebar works as an inbox: agents that need you rise to **Needs you**, and the bell in the title bar counts them.

1. Press ⌘J to open the next agent waiting on you.
2. When you're done with it, press ⇧⌘S to settle it and go to the next one.

A settled agent moves to the **Settled** shelf and MaxHQ stops its process. New activity brings it back. Press ⇧⌘P to pin an agent to the top. ⌘Z undoes the last change while its notice shows.

**Needs you** holds only agents waiting on an answer, such as an approval card or a question. An agent that finished a turn you haven't read stays under its project, marked **Done, unread**, and ⌘J skips it. The bell and the Dock badge still count it while **Finished** is picked under **Dock badge** in Settings, under **Notifications**, as it is at first. Opening the agent clears it.

Right-click an agent, or click its **⋯**, for **Settle**, **Snooze**, **Pin**, **Rename** and more. Select several to act on them at once.

Agents with no activity settle on their own after **Auto-settle after** days, set per device in its **Inbox** card. The switch **Auto-settle when idle** turns that off for one agent.

## Snooze an agent
<!-- id: agents-composer/snooze -->
<!-- covers: settings/sidebar -->

Snooze an agent to hide it until later. It returns to your inbox at the time you pick.

1. Right-click the agent and pick **Snooze**.
2. Pick an option, or **Custom…** to set a date and time or a duration, then click **Snooze**.

The options at first, each with its time on a 24-hour clock:

- **In 1 hour** and **In 3 hours**, such as "In 1 hour (14:30)".
- **This evening (18:00)**.
- **Tomorrow (09:00)**.
- **Next week (Mon 09:00)**.

An option for later today, such as **This evening**, hides once its time has passed. When the agent's account is at its usage limit, the menu also offers **Until the limit resets**.

Snoozed agents wait on the **Snoozed** shelf. **Wake** brings one back early.

To change the options, open Settings, then **Sidebar**, and edit **Snooze options** under **Shelves**. Each option is a time from now, up to 30 days, or a time of day today, tomorrow or on a weekday, which means the next one after today. The menu holds up to eight. **Add option** adds one and **Reset to defaults** brings back the list above. On the same page, **Working shelf** decides whether to **Fold busy agents into a Working shelf**; they come back to the top when they need you.

## Keep going after a usage limit
<!-- id: agents-composer/limit -->
<!-- covers: settings/device/limits settings/device/inbox -->

When an account hits its usage limit, MaxHQ can resume the agent once the limit resets. This works for Claude Code and Codex agents MaxHQ holds, not Cursor or terminal sessions.

1. Right-click the agent and turn on **Continue after the limit**.
2. When the limit hits, the agent shows "Waiting for usage limit reset" with the time it resumes.
3. To go on now with another account, pick **Move to** that account. **Don't resume** cancels the wait.

Each device sets the default per vendor in its **Usage limits** card. It's off by default. The **Inbox** card's **Snooze at the limit** snoozes agents that stop at a limit until it resets.

While an agent waits, the composer reads "Queue a message for when the limit resets".

## Move an agent to another device
<!-- id: agents-composer/move -->

Move an agent when another device suits the work better, such as devbox for a long build.

1. Open the agent's **More** menu and pick **Move to…**.
2. Pick the **Device**, **Account**, **Model** and **Folder**.
3. Click **Move**, or **Stop and move** while a turn runs.

Only a summary of the conversation carries over. The new agent's pane opens once it starts.

## Take over a terminal session
<!-- id: agents-composer/take-over -->

An agent you started in a terminal shows in MaxHQ, but MaxHQ only follows along. Take it over to drive it from MaxHQ instead.

1. Open the agent.
2. Click **Take over** in the banner at the top, and confirm.

MaxHQ stops the session in the terminal and carries on with it here. When the terminal session has already ended, the banner offers to continue it here.

Only Claude Code and Codex sessions can be taken over, and the device's maxd needs to be recent enough. On a sub-agent, **Take over** takes over its lead's session.

## Work in panes and splits
<!-- id: agents-composer/panes -->
<!-- covers: shortcut/split-right shortcut/split-down shortcut/maximize-or-restore-the-pane shortcut/close-the-tab shortcut/reopen-the-last-closed-tab shortcut/next-tab shortcut/previous-tab shortcut/show-the-tab-at-that-place-9-for-the-last shortcut/move-to-the-pane-on-that-side -->

Each agent, note, file or visual you open is a tab in a pane, and you can split the main area to see several at once.

- ⌘D splits right and ⇧⌘D splits down.
- ⌥⌘ with an arrow moves to the pane on that side.
- ⇧⌘] and ⇧⌘[ go to the next and previous tab. ⌘1 to ⌘9 show the tab at that place, 9 for the last.
- ⇧⌘M maximizes the pane or restores it.
- ⇧⌘W closes the tab and ⇧⌘T reopens the last one you closed.

Right-click a tab for **Float**, **Split right**, **Split down**, **Close others** and more. A floating pane has **Dock this pane** to put it back.

## Find in a conversation
<!-- id: agents-composer/find -->
<!-- covers: shortcut/find-in-the-conversation shortcut/next-or-previous-match shortcut/close-find -->

Press ⌘F in an agent's pane to search its conversation.

1. Type what you're looking for in **Find in conversation**.
2. Press Enter for the next match, or Shift-Enter for the previous one.
3. Press Esc, or click **Close find**, when you're done.

The **Next match** and **Previous match** buttons beside the field do the same as Enter and Shift-Enter. To find an agent rather than words inside one, use **Filter** at the top of the sidebar (⌥⌘F).

## Open an agent's folder in an editor
<!-- id: agents-composer/open-in -->
<!-- covers: settings/editor -->

Open the folder an agent works in with **Open in** in its header's **More** menu. MaxHQ opens it in Cursor or VS Code, even in a container on another device.

To choose the editor, open Settings, then **Editor**, and pick a **Default editor** under **Open in**:

- **Last used** opens the one you used last.
- **Cursor** or **VS Code** always opens that one.
- **None** hides the buttons.

The same editor opens files from file rows.

## Let an agent show you things
<!-- id: agents-composer/tools-show -->
<!-- covers: tool/send_attachments tool/open_file tool/show_visual tool/read_agent -->

MaxHQ gives each agent a few tools of its own. These need no permission.

- **Send attachments** shows you files in the conversation, such as a screenshot.
- **Open a file** opens a file in MaxHQ's file viewer.
- **Show a visual** shows a web page, such as a chart, diagram or mockup.
- **Read an agent** reads another agent's conversation, such as one you attached to your message as a pill. While the device's **Other agents** card allows reading, as it does by default, it reads any agent **List agents** names.

Claude Code sees these tools in an MCP server named maxhq. Codex sees them with a maxhq_ prefix, such as maxhq_open_file.

## Let an agent run other agents
<!-- id: agents-composer/tools-run-agents -->
<!-- covers: tool/list_targets tool/list_agents tool/start_agent tool/send_agent tool/wait_agent tool/interrupt_agent tool/move_agent -->

An agent can start and manage other agents once you allow it. Open its **More** menu and pick **Allow to run agents**; later, **Change what it can run** narrows or widens that.

With the permission, the agent gets these tools:

- **List where agents can start**: devices, vendors, accounts, workspaces and models.
- **List agents** and **Start an agent**.
- **Message an agent** and **Wait for agents**, for up to an hour.
- **Stop an agent**.
- **Move an agent to another device**, once you approve the move.

**List where agents can start**, **List agents** and **Wait for agents** only read, so they need no permission while the device's **Other agents** card allows reading, as it does by default.

Settings, then **Agent tools**, sets what new permissions allow, such as how deep agents may nest and how many run at once.

## Let an agent schedule tasks
<!-- id: agents-composer/tools-tasks -->
<!-- covers: tool/schedule_task tool/list_tasks tool/delete_task tool/run_task_now -->

An agent allowed to run agents can also set up work that starts later on its own.

- **Schedule a task** runs a prompt with nobody watching: on a timer, when a webhook gets a request, or when a GitHub repository has new events.
- **List scheduled tasks** shows the ones there are.
- **Delete a scheduled task** removes one.
- **Run a scheduled task now** starts one at once.

You can see and change scheduled tasks in Settings, under **Scheduled tasks**.

## Let an agent work with pull requests
<!-- id: agents-composer/tools-pull-requests -->
<!-- covers: tool/link_pull_request tool/unlink_pull_request tool/list_pull_requests tool/watch_pull_request tool/unwatch_pull_request -->

Agents can tie themselves to pull requests and follow them, with no permission needed.

- **Link a pull request** ties a pull request to the agent, and **Unlink a pull request** undoes it. You can also pick **Link a pull request** in the agent's **More** menu.
- **List your pull requests** lists the ones linked to the agent, with each one's state and checks.
- **Watch a pull request** wakes the agent when checks fail, all checks pass, comments or reviews arrive, or the pull request conflicts.
- **Stop watching a pull request** ends that.

## Let an agent test on a leased machine
<!-- id: agents-composer/tools-test-machines -->
<!-- covers: tool/list_test_hosts tool/lease_open tool/lease_run tool/lease_screenshot tool/lease_install tool/lease_close -->

An agent can borrow a test machine, such as an iOS Simulator, an Android emulator, a Linux container or a macOS VM, once you allow the kinds it may lease.

- **List test hosts** shows which kinds each device offers.
- **Lease a test machine** starts one for the agent.
- **Run a command on a leased machine** and **Screenshot a leased machine** drive it.
- **Install an app on a leased machine** installs an .app or .ipa on a Simulator, an .apk on an emulator, or any file or folder on Linux, up to 4 GiB.
- **End a lease** gives the machine back.

## Let an agent keep notes
<!-- id: agents-composer/tools-notes -->
<!-- covers: tool/list_notes tool/read_note tool/create_note tool/edit_note -->

Agents can read and write the notes of the project they work in, with no permission needed.

- **List notes** lists the project's notes.
- **Read a note** reads one.
- **Create a note** adds one.
- **Edit a note** changes one.

To point an agent at a note, type `@` in the composer and pick it. See [Notes](notes.md).
