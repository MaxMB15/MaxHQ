<!--
SPDX-License-Identifier: MIT
Copyright (c) 2026 Max Boksem
-->

# The dispatcher

The box at the top of the window takes a request in plain words and carries it out, with risky changes in a plan you confirm, or answers a question about MaxHQ from this guide.

## What the dispatcher is
<!-- id: dispatcher/what-it-is -->

The dispatcher is an agent on your Mac that runs MaxHQ for you. It reads your projects and agents, makes the changes you ask for, and asks first for risky ones. It can't change your files.

You type a request in **Ask the dispatcher** at the top of the window, such as "Start an agent in example-app to fix the failing login test". The dispatcher reads what it needs, asks you about anything the request leaves open, then does it or shows a plan under the box. Nothing in a plan runs until you confirm.

It also answers questions about MaxHQ itself, such as where a setting lives, from this guide.

On a phone, the box uses the dispatcher of the Mac it's paired with.

## Ask the dispatcher to do something
<!-- id: dispatcher/ask -->

Describe the work in the box at the top, and the dispatcher does it or plans it.

1. Click the box at the top that reads "Ask anything, or describe a task for any project".
2. Type what you want, such as "Ask the agent on devbox to rebase on main", and press Enter. Shift-Enter starts a new line.
3. If it asks which project, account or person you mean, pick an answer in the panel.
4. Read what it did, or the plan it shows, then confirm or dismiss the plan.

Add context as pills: `@` picks an agent on any device and `#` a pull request or issue, and you can drag an agent or a quote onto the box. The dispatcher passes them on to the agents it starts, so they get the same context. It can't read notes itself, and a note only reaches an agent in the note's project.

## Confirm or dismiss a plan
<!-- id: dispatcher/plan -->
<!-- covers: tool/submit_plan -->

A plan lists what the dispatcher would do, one line per item, and waits for you. The dispatcher hands it over with **Submit a plan**.

An item can start an agent, message an agent, make a scheduled task, move an agent to another device, or make a change that needs your say, such as deleting an agent. Each line says why, and how the agent would run, such as "Runs in Auto". Click an item to see its details, such as the first message or a task's schedule.

- Click **Confirm**, or press Enter in the empty box, to run the plan.
- Click **Dismiss**, or press Esc, to drop it and start over.

**Plan results** then says how each item went, with **Open agent** or **Open task** to go to what it made. Items run in order, and when one fails, the plan stops there.

## Changes the dispatcher makes itself
<!-- id: dispatcher/acts -->
<!-- covers: tool/manage_agent tool/manage_project tool/manage_workspace tool/manage_task tool/manage_device tool/manage_permission -->

Ask for a change and the dispatcher makes it at once when it's safe, such as renaming an agent, snoozing it, changing an idle agent's model, hiding a project or turning a scheduled task off. It also starts and messages agents and makes scheduled tasks on this Mac when you ask. A card under its answer says what it did, such as "Renamed Fix login to Auth fix", and names the device when the change was on another one.

These wait in a plan for you to confirm, even when you asked for them:

- deleting an agent or a scheduled task
- stopping an agent that's busy, or stopping a workspace
- letting an agent run other agents, or taking that back
- updating the agent CLIs
- moving an agent, or giving one full access
- a change you didn't ask for
- more than five changes for one request

Sign-ins, network access, Cursor usage and the **Act directly** switch stay with you, and the dispatcher never touches access keys or tokens.

It makes its changes with **Manage an agent**, **Manage a project**, **Manage a workspace**, **Manage a scheduled task**, **Manage a device** and **Manage what an agent may run**.

## Undo a change the dispatcher made
<!-- id: dispatcher/undo -->

Click **Undo** on the card to put the change back. It works once, and the card then says **Undone**.

- Undo works for renaming an agent, inbox changes such as snoozing or settling, an agent's mode, model and effort, hiding a project, a project's settings, turning a task on or off, and device settings.
- Starting, messaging, stopping or deleting, running a task now, workspaces, permissions and updates have no Undo.
- Undo greys out once maxd restarts or once something else changed the same thing since. Hover it to see "This change can't be undone any more". It reaches back over the dispatcher's last 50 changes.

## Turn off Act directly
<!-- id: dispatcher/act-directly -->
<!-- covers: settings/dispatcher -->

Act directly is on by default. Turn it off and every change waits in a plan until you confirm it.

1. Open Settings (⌘,) and pick **Dispatcher**.
2. Under **Changes**, turn off **Act directly**.

With it on, deleting, stopping busy agents and granting permissions still ask. This Mac's maxd keeps the switch. An older maxd greys it out and says to update it first.

## Ask how to do something in MaxHQ
<!-- id: dispatcher/how-to -->
<!-- covers: tool/search_guide tool/read_guide -->

Ask the dispatcher where something is or how to do it, and it answers from this guide, with a link that opens the right view.

1. Type a question in **Ask the dispatcher**, such as "How do I split a pane?" or "Where do I set the default editor?".
2. Read the short steps it gives back.
3. Click the link at the end to open that view, such as the Editor settings.

The dispatcher looks things up with **Search the guide**, then **Read a guide section** when a short excerpt isn't enough. When the guide has nothing on it, it says so rather than guess. A question needs no plan.

## What the dispatcher can read
<!-- id: dispatcher/reads -->
<!-- covers: tool/list_repo_pull_requests tool/read_agent tool/list_targets tool/list_agents tool/list_tasks -->

The dispatcher has tools that only read, so it can plan with what's really there.

- **List where agents can start** gives your projects, their folders and workspaces, and the accounts signed in on each device, with their models.
- **List agents** and **Read an agent** show your agents and their conversations.
- **List scheduled tasks** shows your scheduled tasks.
- **List a project's pull requests** shows a project's open pull requests and their authors, which is how it finds a person's GitHub login from their name.

What it reads from repositories, pull requests and other agents is data to it, not instructions.

## Keep each topic in its own chat
<!-- id: dispatcher/chats -->
<!-- covers: tool/name_chat tool/switch_chat tool/find_chats -->

The dispatcher keeps each topic in a chat of its own, so a follow-up builds on what you asked before.

- What you ask within 30 minutes of your last message, or of the dispatcher's last answer, stays in the same chat. After a longer gap a new chat starts, and the dispatcher looks over your recent chats to carry on the right one.
- The dispatcher names each chat with **Name this chat**: a short title, a line about it and its project. A chat without a name yet shows as "Chat 1", "Chat 2" and so on.
- When a request is about another topic, the dispatcher moves it with **Move the message to another chat**, and that chat answers it. The chat it left says "Moved your message to" and the other chat's title, with **Keep here** to bring it back. The chat it went to says "Moved your message here from", with **Move back**.
- With **Find earlier chats** it looks further back, through the titles, summaries and full text of every chat, archived ones too.

## Find, rename and archive chats
<!-- id: dispatcher/chats-list -->

The top bar of the dispatcher's panel has **Save to Notes**, **Chats**, **New conversation** and **Open as pane**.

1. Click **Chats** for the list, grouped under **Today**, **This week**, **Earlier** and **Archived**.
2. Type in **Search chats** to find one by its title or summary. The best fits show first, under **Matches**.
3. Click a chat to carry on there. What you ask next goes to it.

Hover a chat, or move to it with Tab, for its buttons:

- **Rename chat**: type a title and press Enter. The dispatcher keeps a title you gave.
- **Save to Notes**: the chat writes a note of what was asked, decided and found, in its project. A chat that isn't about a project asks you which one. **Save to Notes** in the top bar saves the chat you're in.
- **Archive** moves it under Archived, and **Unarchive** brings it back. A chat you haven't used for 7 days moves there by itself.
- **Delete** asks first. MaxHQ stops the chat, and the dispatcher won't list or find it again. Its CLI's transcript stays.

**New conversation** starts a fresh chat. After you dismiss a plan, what you ask next starts a new chat too. **Open as pane** keeps the conversation beside your agents.

## Choose the account it runs as
<!-- id: dispatcher/account -->
<!-- covers: settings/dispatcher -->

The dispatcher runs on one of this Mac's Claude or Codex accounts. The panel's title says which, such as "Dispatcher, as" and the account.

1. Open Settings (⌘,) and pick **Dispatcher**.
2. Under **Account**, pick one in **Runs as**.

Without a pick, it takes the first Claude account, else the first Codex one. With no account on this Mac, the page asks you to sign in to Claude or Codex on this Mac first.
