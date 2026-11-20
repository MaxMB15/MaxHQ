<!--
SPDX-License-Identifier: MIT
Copyright (c) 2026 Max Boksem
-->

# The dispatcher

The box at the top of the window takes a request in plain words and turns it into a plan you confirm, or answers a question about MaxHQ from this guide.

## What the dispatcher is
<!-- id: dispatcher/what-it-is -->

The dispatcher is an agent on your Mac that turns a request into a plan. It can read your projects and agents, but it can't change files or start anything itself.

You type a request in **Ask the dispatcher** at the top of the window, such as "Start an agent in example-app to fix the failing login test". The dispatcher reads what it needs, asks you about anything the request leaves open, then shows a plan under the box. **Nothing runs until you confirm.**

It also answers questions about MaxHQ itself, such as where a setting lives, from this guide.

On a phone, the box uses the dispatcher of the Mac it's paired with.

## Ask the dispatcher to do something
<!-- id: dispatcher/ask -->

Describe the work in the box at the top, and the dispatcher plans it.

1. Click the box at the top that reads "Ask anything, or describe a task for any project".
2. Type what you want, such as "Ask the agent on devbox to rebase on main", and press Enter. Shift-Enter starts a new line.
3. If it asks which project, account or person you mean, pick an answer in the panel.
4. Read the plan it shows, then confirm or dismiss it.

Add context as pills: `@` picks an agent on any device and `#` a pull request or issue, and you can drag an agent or a quote onto the box. The dispatcher passes them on to the agents it starts, so they get the same context. It can't read notes itself, and a note only reaches an agent in the note's project.

## Confirm or dismiss a plan
<!-- id: dispatcher/plan -->
<!-- covers: tool/submit_plan -->

A plan lists what the dispatcher would do, one line per item, and waits for you. The dispatcher hands it over with **Submit a plan**.

An item can start an agent, message an agent, make a scheduled task, or move an agent to another device. Each line says why, and how the agent would run, such as "Runs in Auto". Click an item to see its details, such as the first message or a task's schedule.

- Click **Confirm**, or press Enter in the empty box, to run the plan.
- Click **Dismiss**, or press Esc, to drop it and start over.

**Plan results** then says how each item went, with **Open agent** or **Open task** to go to what it made. Items run in order, and when one fails, the plan stops there.

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

## Keep or start a conversation
<!-- id: dispatcher/conversation -->

The dispatcher remembers the conversation, so a follow-up can build on what you asked before.

- Click **New conversation** in the panel to start fresh. After you dismiss a plan, what you ask next starts a new conversation too.
- Click **Open as pane** to keep the conversation open beside your agents.

## Choose the account it runs as
<!-- id: dispatcher/account -->
<!-- covers: settings/dispatcher -->

The dispatcher runs on one of this Mac's Claude or Codex accounts. The panel's title says which, such as "Dispatcher, as" and the account.

1. Open Settings (⌘,) and pick **Dispatcher**.
2. Under **Account**, pick one in **Runs as**.

Without a pick, it takes the first Claude account, else the first Codex one. With no account on this Mac, the page asks you to sign in to Claude or Codex on this Mac first.
