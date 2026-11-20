<!--
SPDX-License-Identifier: MIT
Copyright (c) 2026 Max Boksem
-->

# Skills

How MaxHQ keeps one library of skills, which skills every agent gets, and how each project picks its own set and sees what each CLI loads.

## Add a source to the skill library
<!-- id: skills/add-source -->
<!-- covers: settings/skills -->

The skill library holds sources: git repos or folders of skills that projects pick from. Add one in Settings › Skills, under Add a source.

1. Open Settings and pick Skills.
2. Under Add a source, fill in Repo or folder with owner/repo, a git URL or a folder path. The folder button opens a picker.
3. Give it a Name, and a Branch or tag when you don't want its default branch.
4. Turn on A third party's skills when someone else writes them. MaxHQ pins them until you update them.
5. Click Add source.

The source shows under Sources, marked Yours or Third party. Fetch brings in its newest skills, Push appears when a source of yours has changes to send, and Remove takes it out.

## Choose the skills every agent gets
<!-- id: skills/every-agent -->
<!-- covers: settings/skills -->

The Every agent section of Settings › Skills lists the Skills every agent gets. MaxHQ keeps them global on each device, so a project can't take them away.

Two skills are in that list: unslop from the library, and propose-skill-change, which MaxHQ ships itself. The second one tells agents how to suggest a change to a project's skills instead of editing them in place. You review those changes on the project's Skills tab.

A project can't add a skill every agent already gets. Its Library list says "Every agent already gets" that skill instead of offering Add.

## Move your global skills into the library
<!-- id: skills/tidy-global -->
<!-- covers: settings/skills -->

Global skills sit in each CLI's own skills folder on a device. Tidy global skills moves your own ones into the library, so you pick them per project.

1. Open Settings › Skills and find the Global skills section.
2. Click Tidy global skills on the device's row.
3. Read the dialog. It lists what moves into the library, what joins as sources, where you pick one copy, links that appear and copies that go to the backup.
4. Click Apply.

Nothing is deleted. Moved folders go to a dated backup folder, and Undo on the device's row puts them back. MaxHQ writes only skill folders and links, never your CLI settings files. Hiding a plugin or system skill stays your job, in that CLI's own settings.

## Move the skill library to another device
<!-- id: skills/library-device -->
<!-- covers: settings/device/library -->

One device holds the skill library. To move it, turn on Holds the skill library on the Skill library card of the device that should hold it.

1. Open Settings and pick the device under Devices.
2. Find the Skill library card.
3. Turn on Holds the skill library.

The library and its sources move to that device. When no device holds one yet, Settings › Skills says it comes with a newer maxd, so update maxd on a device first (see [Updates](updates.md)).

## Pick a project's skills
<!-- id: skills/project -->
<!-- covers: settings/project-skills -->

A project's Skills tab picks which library skills its agents get. Open it from the project's ⋯ menu or right-click in the sidebar, then Project settings… and the Skills tab.

1. In Library, search for a skill and click Add. It joins This project.
2. To copy another project's set, pick that project in Add from project.
3. Click Apply to put the set into the project's checkouts. "Not in the checkouts yet" shows until you do.

Each skill in This project has Update, Pin and Remove. A pinned skill stays at its version until you update it, and Update all brings every unpinned one up to date. With more than one device, the switcher at the top picks which device's checkouts you change.

## Commit a project's skills to the repo
<!-- id: skills/commit -->
<!-- covers: settings/project-skills -->

Turn on Commit the skills on a project's Skills tab when everyone who clones the repo should get them. Off, they stay in your checkouts only.

1. Open the project's Skills tab.
2. Under This project, turn on "Commit the skills, so everyone who clones the repo gets them".
3. Click Apply.

When Apply would push a CLI past the length of its skill listing, the tab warns you first. That CLI then drops skill descriptions, so trim the set or check What each CLI sees.

## Review a skill change an agent proposes
<!-- id: skills/proposals -->
<!-- covers: settings/project-skills -->

Agents can't edit a project's skills directly, since MaxHQ puts them back before each launch. They propose a change instead, and you approve or reject it in Proposals.

1. A skill change shows in the sidebar, and in the Proposals section of the project's Skills tab as "From" the agent that made it.
2. Click Review to see the diff.
3. Click Approve to keep it in the library, Approve and push to also push its source, or Reject.

For a third party's skill, approving forks it. Give the copy a name in Fork name in your personal source. When a project skill's files change in a checkout, the tab says so and Restore puts them back.

## See which skills each CLI loads
<!-- id: skills/what-cli-sees -->
<!-- covers: settings/project-skills -->

What each CLI sees, on a project's Skills tab, lists the skills Claude, Codex, Cursor and ACP agents find in a checkout, and where each one comes from. Click Rescan to look again.

Badges name the origin: Project, Every agent, Global, Plugin, System, Synced or Vendored. Marks warn when a global skill of the same name runs instead, when the same skill sits in another folder, or when another installer owns it.

A meter counts the characters of each CLI's skill listing. Past it, the CLI drops descriptions. To hide a Claude skill, add it to skillOverrides in Claude's settings or use /skills. When the skills change, a running agent picks them up on its next message.

## Add the devflow pack to a project
<!-- id: skills/devflow-pack -->
<!-- covers: settings/project-skills -->

The Devflow pack row on a project's Skills tab installs a devflow pack into each checkout. Pick a pack source in its menu, or None to remove it.

1. Open the project's Skills tab.
2. In Devflow pack, pick the pack's source.
3. Watch each checkout's line: Installing, then Installed. Click Retry on one that failed.

Update brings the pack to its newest version. Picking None opens Remove the devflow pack, which takes the pack, its links and its exclude lines out of each checkout. It leaves files such as CLAUDE.md, AGENTS.md and .mcp.json for you to delete.
