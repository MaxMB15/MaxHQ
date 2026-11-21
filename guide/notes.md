<!--
SPDX-License-Identifier: MIT
Copyright (c) 2026 Max Boksem
-->

# Notes

Each project can keep notes in Markdown, for you and its agents. This page covers making, editing and finding them, and where they live.

## Create a note
<!-- id: notes/create -->
<!-- covers: view/note -->

Make a note from the project's menu in the sidebar. It opens in a pane, ready to type in.

1. Right-click the project in the sidebar, or click its **⋯** button.
2. Pick **New note**.
3. Type a title in the **Title** field, then write the note below it.

Once a project has notes, its group in the sidebar shows a **Notes** entry. Right-click that entry, or any note under it, for **New note** as well.

## Open a note
<!-- id: notes/open -->

Find a project's notes under its **Notes** entry in the sidebar.

1. Click **Notes** under the project to fold it open. It shows how many notes there are, and one row per note.
2. Click a note's row to open it in a pane, or to go to its pane when it's already open.

A note opens in a tab like an agent does, so you can split the window and keep a note beside the agent working from it. See [Agents and the composer](agents-composer.md).

## Write and format a note
<!-- id: notes/edit -->
<!-- covers: shortcut/move-the-lines-up-or-down-in-a-note shortcut/copy-the-lines-up-or-down-in-a-note -->

A note is a Markdown editor that saves as you type. The pane's header says where saving stands, such as **Saved** or **Saving**.

- Lists continue on Enter. Tab nests an item and Shift-Tab moves it back out.
- Task boxes tick on a click.
- Option and an arrow moves the caret's line, or every selected line, up or down. Option-Shift and an arrow copies them.
- ⌘-click opens a link.
- **Edit the source**, the **</>** button, shows a block's Markdown until the caret leaves it.

Spelling follows **Check spelling** in Settings, under **Editor**.

To put a visual an agent showed you into a note, use **Add to note** on the visual, or drop it on the note's row in the sidebar.

## See and restore earlier versions
<!-- id: notes/history -->

Every save of a note is kept as a version, so you can go back to an earlier one.

1. Click **History** in the note's header. Versions are listed newest first.
2. Pick one to read it. It shows read-only in place of the editor.
3. Click **Restore** to save that version as the newest one, or **Back to current** to return.

Restoring never removes a version, so you can undo a restore the same way.

## Rename or delete a note
<!-- id: notes/rename-delete -->

Rename or delete a note from its row in the sidebar or from its pane.

- To rename, right-click the note's row and pick **Rename**, or change the **Title** field in its pane.
- To delete, right-click the row and pick **Delete…**, or click **Delete** in the pane's header. MaxHQ asks first.

A delete reaches every device. If a note you have open is deleted elsewhere, its pane says so and offers **Bring it back**.

## Where notes live
<!-- id: notes/storage -->

Notes live with maxd on your devices, outside git, so they never show up in a repository or a commit.

- Each device keeps every version of a project's notes in its own maxd database.
- Devices that both hold a project share its notes, and a change on one reaches the others.
- A device keeps a project's notes when it has a workspace of that project, or already holds its notes.

The Mac app reads and saves notes through this Mac's maxd. While that's down, a note from another device shows **Read-only**, with a way to start maxd again.

## Share a note with an agent
<!-- id: notes/agents -->

Agents can read and write the notes of the project they work in, with no permission needed. An agent in a container asks you once first, since the notes are shared outside it. An agent in Plan only mode reads them but can't change them.

- To point an agent at a note, type `@` in its composer and pick the note. It goes in as a pill.
- An agent can list, read, create and edit its project's notes on its own. See [Let an agent keep notes](agents-composer.md).
