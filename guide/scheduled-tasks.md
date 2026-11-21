<!--
SPDX-License-Identifier: MIT
Copyright (c) 2026 Max Boksem
-->

# Scheduled tasks

How to send a prompt to an agent on a schedule, from a webhook or on GitHub events, and how to check what each task did.

## Make a scheduled task
<!-- id: scheduled-tasks/new -->
<!-- covers: settings/scheduled-tasks -->

A scheduled task sends a prompt to an agent each time it runs. Its device runs it, so it works with your Mac closed. Click New task in Settings › Scheduled tasks.

1. Open Settings and pick Scheduled tasks.
2. Click New task.
3. Give it a Name, and write the Prompt: what the agent should do each time.
4. Under When it runs, pick how it starts in Runs.
5. Under Where it goes, pick the agent and the device.
6. Click Make task.

Tasks show grouped by device, each with its next and last run. A switch turns one on or off, and Edit opens it again. Delete removes a task with its runs and requests.

## Run a task every few minutes or at set times
<!-- id: scheduled-tasks/schedule -->
<!-- covers: settings/scheduled-tasks -->

Set Runs to Every for a task that repeats after a number of minutes, or to At for set times of day. Both sit under When it runs in the task's editor.

1. Open the task with Edit, or start one with New task.
2. Under When it runs, set Runs to Every and fill in the minutes, or set it to At.
3. For At, fill in the Time and pick the Days. No days picked runs it every day. The time is in the device's time zone, which the editor names.
4. Click Save task.

The task's row shows its next run. To run it once right now, click Run now.

## Start a task from a webhook
<!-- id: scheduled-tasks/webhook -->
<!-- covers: settings/scheduled-tasks -->

Set Runs to Webhook to start the task when a program sends a request to its URL. The URL shows once, after you save, so copy it then.

1. Set Runs to Webhook and keep Take requests on.
2. To check who sent it, turn on Check a signature and fill in the Signature header, Encoding, Prefix and Signing secret. Until the secret is set, the webhook refuses every request.
3. Turn on "Hold requests up to 24 hours while the target device is offline" if requests may come while it's away.
4. Click Make task, then Copy URL in the window that follows.

Placeholders such as {{body.action}} in the prompt take the request's values. Turning Take requests off turns requests away.

## Get a new webhook URL
<!-- id: scheduled-tasks/rotate -->
<!-- covers: settings/scheduled-tasks -->

Click Rotate URL on a webhook task's row in Settings › Scheduled tasks to replace its URL. The current URL stops working at once.

1. Open Settings › Scheduled tasks.
2. Find the webhook task and click Rotate URL.
3. Confirm, then copy the new URL from the window that shows it.

MaxHQ shows each URL only once. If you lose it, rotate again and give the sender the new one.

## Reach webhooks from outside the device
<!-- id: scheduled-tasks/webhook-address -->

By default only programs on the device reach its webhooks. Pick Tailscale Funnel or Your own address in the device's Webhook address row to let other senders in.

1. Open Settings › Scheduled tasks and find the device's Webhook address row.
2. In Reached, pick This device only, Tailscale Funnel or Your own address.
3. For Tailscale Funnel, confirm with Use Funnel. maxd adds one path per webhook that's on, and anyone with a URL can reach it.
4. For Your own address, paste an https address of a tunnel or proxy that points at the device's webhook port. maxd can't check it for you.

Tailscale is optional. Your own address works without it.

## Filter the requests a webhook takes
<!-- id: scheduled-tasks/requests -->
<!-- covers: settings/scheduled-tasks -->

Requests it takes, in a webhook task's editor, decides which requests run the task. Every request runs it until you add a filter.

- Filters: click Add filter, name a field, and pick a test such as equals or starts with. All filters must match.
- Replay key: a placeholder such as {{headers.x-github-delivery}}. A request whose key MaxHQ saw in the last 7 days doesn't run, and History lists it as Duplicate. A run that fails lets the same key run again.
- Coalesce key: requests whose key fills in the same, such as {{body.pull_request.number}}, wait for one run, and the prompt gets each of them. A key with no placeholder, such as all, puts every request in one window. A request without the key's field runs on its own.
- Coalesce window, seconds: how long the window stays open, 120 by default, from 10 to 3600. It counts from the first request.
- Restart the window with each request: counts from the last request instead, up to Longest wait, seconds after the first, 7200 by default and 24 hours at most.
- A task holds up to 20 open windows. Past that, a new key's request shows as Queue full and doesn't run.
- Skip requests older than, minutes, and Runs a day cap stale requests and daily runs.

Fields in the prompt lists each placeholder the prompt uses, marked Free text, Structural or Structural parts. Numbers, logins, actions and addresses fill in as they are. Free text, such as a title or a comment, comes from whoever wrote it, so it fills in empty until you tick it. A ticked one fills in between `<webhook-data-…>` tags, with a note that it's untrusted data.

A webhook made before this list fills in every field, and says "This prompt fills in every field of a request". Use the safe default turns that into the list, with free text left out.

## Run a task on GitHub events
<!-- id: scheduled-tasks/github -->
<!-- covers: settings/scheduled-tasks -->

Set Runs to GitHub events to start a task when something happens in a repo, such as a new pull request. The repo must be in the GitHub App first.

1. Add the repo to the GitHub App in Settings › GitHub App.
2. In the task's editor, set Runs to GitHub events.
3. Pick the Repository and tick the Events.
4. Keep "Only for the repo's owners, members and collaborators" on, so outsiders can't run it.
5. Click Make task.

The row says whether events come instantly through a webhook or the device checks every minute. Requests it takes works here too.

## Choose where a task's agent runs
<!-- id: scheduled-tasks/where -->
<!-- covers: settings/scheduled-tasks -->

Where it goes, in the task's editor, picks the agent that gets the prompt and the device that runs the task. Sends to picks the agent and Lives on picks the device.

1. In Sends to, pick A new agent, started for each run, or an agent you already have.
2. For A new agent, set it up in the agent pickers that appear, as you would for any new agent.
3. In Lives on, pick the device that runs the task. You can't change it after saving.

An agent that a task sends to shows the task under Automations in its header, with Run now and an on/off switch.

## See what a task did
<!-- id: scheduled-tasks/history -->
<!-- covers: settings/scheduled-tasks -->

Click History on a task's row to see its runs and, for webhooks, every request it got. Each run says whether it ran, failed, stopped or was missed.

1. Open Settings › Scheduled tasks.
2. Click History on the task.
3. Read the Runs tab, and the Deliveries tab for a webhook or GitHub task. Click Refresh for the newest.

A delivery that didn't run says why: Duplicate, Coalesced, Queue full, Bad signature, Task paused or Rate limited.

## Approve a task an agent made
<!-- id: scheduled-tasks/approve -->
<!-- covers: settings/scheduled-tasks -->

A task an agent makes, turns on or changes stays off until you approve it. Its row says "Waits for your approval".

1. Open Settings › Scheduled tasks.
2. Find the task marked Waits for your approval and read its Prompt and When it runs with Edit.
3. Turn it on, or click Save task, to approve it. Leave it off, or Delete it, to refuse.
