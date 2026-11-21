// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The keyboard shortcuts the list shows, in groups. The pane, inbox and
// find shortcuts come from `SHORTCUTS`, so one added there shows up without
// a change here, under Other until it gets words. The keys that other parts
// of the app listen for, such as Cmd+N and the composer's keys, are written
// out here. Enter and Cmd+Enter read as this Mac's pick for a working agent.

import { type BusySend, DEFAULT_BUSY_SEND } from "@maxhq/shared/composer/send";
import { t } from "@maxhq/shared/i18n";

import { keyHint, SHORTCUTS, type Shortcut, terminalFind } from "@/layout/shortcuts";
import { onMac } from "@/native/platform";

/** The groups, in the order the list shows them. */
export const SHORTCUT_GROUPS = [
	"General",
	"Agents",
	"Sidebar",
	"Inbox",
	"Panes",
	"Find",
	"Composer",
	"Notes",
	"Other",
] as const;

export type ShortcutGroup = (typeof SHORTCUT_GROUPS)[number];

/** One line of the list. */
export type ShortcutRow = {
	/** What the key does. */
	readonly what: string;
	/** The keys as a menu writes them, such as ⇧⌘D. More than one means any of them. */
	readonly keys: readonly string[];
	readonly group: ShortcutGroup;
	/** Whether the key works only in projects mode, where the panes show. */
	readonly projectsOnly: boolean;
};

type Id = keyof typeof SHORTCUTS;

// A line of the list: a shortcut from `SHORTCUTS` by its name, or a key
// another part of the app listens for. A key that holds Command and works
// anywhere names itself, so if `SHORTCUTS` gets the same key later, that
// entry takes this line rather than showing twice.
type Line = {
	readonly what: string;
	readonly group: ShortcutGroup;
	readonly projectsOnly?: boolean;
} & ({ readonly id: Id } | { readonly keys: readonly string[]; readonly anywhere?: Shortcut });

// A key on its own, as a Mac writes it and as other systems do.
type Plain = { readonly mac: string; readonly other: string };
type Mods = { readonly shift?: boolean; readonly alt?: boolean };

const ENTER: Plain = { mac: "⏎", other: "Enter" };
const UP: Plain = {
	mac: "↑",
	get other() {
		return t("Up");
	},
};
const DOWN: Plain = {
	mac: "↓",
	get other() {
		return t("Down");
	},
};
const ESCAPE: Plain = { mac: "Esc", other: "Esc" };

const plain = (key: Plain, mods: Mods = {}): string =>
	onMac()
		? `${mods.alt ? "⌥" : ""}${mods.shift ? "⇧" : ""}${key.mac}`
		: `${mods.alt ? "Alt+" : ""}${mods.shift ? "Shift+" : ""}${key.other}`;

// Command, or Control off a Mac, with a key that has no letter.
const command = (key: Plain, mods: Mods = {}): string =>
	keyHint({ label: onMac() ? key.mac : key.other, code: "", ...mods });

const NEW_AGENT: Shortcut = { label: "N", code: "KeyN" };
const SETTINGS: Shortcut = { label: ",", code: "Comma" };
const ATTACH: Shortcut = { label: "U", code: "KeyU" };

// The list in order. The projects-only shortcuts are the ones `commandFor`
// checks after the mode, and the panes' arrows and numbers.
const lines = (pick: BusySend): readonly Line[] => [
	{ what: t("Show keyboard shortcuts"), group: "General", id: "shortcuts" },
	{ what: t("Open settings"), group: "General", keys: [keyHint(SETTINGS)], anywhere: SETTINGS },
	{ what: t("New agent"), group: "Agents", keys: [keyHint(NEW_AGENT)], anywhere: NEW_AGENT },
	{ what: t("Open the next agent waiting on you"), group: "Agents", id: "nextWaiting" },
	{ what: t("Start the agent, in the New agent form"), group: "Agents", keys: [command(ENTER)] },
	{
		what: t("Pick the main option, on an approval card with focus"),
		group: "Agents",
		keys: [plain(ENTER)],
	},
	{
		what: t("Pick that option, on a question card with focus"),
		group: "Agents",
		keys: [plain({ mac: "1–9", other: "1–9" })],
	},
	{ what: t("Show or hide the sidebar"), group: "Sidebar", id: "sidebar" },
	{ what: t("Filter the agents"), group: "Sidebar", id: "filter" },
	{ what: t("Open the first agent the filter shows"), group: "Sidebar", keys: [plain(ENTER)] },
	{ what: t("Clear the filter, then leave it"), group: "Sidebar", keys: [plain(ESCAPE)] },
	{
		what: t("Resize the sidebar, on its edge with focus"),
		group: "Sidebar",
		keys: [plain({ mac: "←→", other: t("Left/Right") })],
	},
	{
		what: t("Move between the agents, on an agent with focus"),
		group: "Sidebar",
		keys: [plain({ mac: "↑↓", other: t("Up/Down") })],
	},
	{ what: t("Open the agent, on an agent with focus"), group: "Sidebar", keys: [plain(ENTER)] },
	{
		what: t("Settle and go to the next agent waiting, or settle the picked ones"),
		group: "Inbox",
		id: "settle",
	},
	{ what: t("Pin or unpin the chosen agents"), group: "Inbox", id: "pin" },
	{ what: t("Undo the last change, while its notice shows"), group: "Inbox", id: "undo" },
	{
		what: t("Move to the pane on that side"),
		group: "Panes",
		projectsOnly: true,
		keys: [keyHint({ label: onMac() ? "←→↑↓" : "Arrows", code: "", alt: true })],
	},
	{ what: t("Next tab"), group: "Panes", projectsOnly: true, id: "nextTab" },
	{ what: t("Previous tab"), group: "Panes", projectsOnly: true, id: "previousTab" },
	{
		what: t("Show the tab at that place, 9 for the last"),
		group: "Panes",
		projectsOnly: true,
		keys: [keyHint({ label: "1–9", code: "" })],
	},
	{ what: t("Split right"), group: "Panes", projectsOnly: true, id: "splitRight" },
	{ what: t("Split down"), group: "Panes", projectsOnly: true, id: "splitDown" },
	{ what: t("Maximize or restore the pane"), group: "Panes", projectsOnly: true, id: "maximize" },
	{ what: t("Close the tab"), group: "Panes", projectsOnly: true, id: "close" },
	{ what: t("Reopen the last closed tab"), group: "Panes", id: "reopen" },
	{ what: t("Show or hide the terminal"), group: "Panes", projectsOnly: true, id: "terminal" },
	{
		what: t("Jump to the previous or next command, in a terminal"),
		group: "Panes",
		projectsOnly: true,
		// Control alone with an arrow goes to the shell off a Mac.
		keys: [command(UP, { shift: !onMac() }), command(DOWN, { shift: !onMac() })],
	},
	{ what: t("Find in the conversation"), group: "Find", projectsOnly: true, id: "find" },
	{
		what: t("Find in the terminal"),
		group: "Find",
		projectsOnly: true,
		// A Mac uses the line above's key, so the line shows only elsewhere.
		keys: onMac() ? [] : [keyHint(terminalFind())],
	},
	{
		what: t("Next or previous match"),
		group: "Find",
		projectsOnly: true,
		keys: [plain(ENTER), plain(ENTER, { shift: true })],
	},
	{ what: t("Close find"), group: "Find", projectsOnly: true, keys: [plain(ESCAPE)] },
	{
		what:
			pick === "steer"
				? t("Send, or steer while the agent works")
				: t("Send, or queue while the agent works"),
		group: "Composer",
		keys: [plain(ENTER)],
	},
	{
		what:
			pick === "steer"
				? t("Queue until the agent finishes")
				: t("Send at once to steer the agent"),
		group: "Composer",
		keys: [command(ENTER)],
	},
	{
		what: t("New line"),
		group: "Composer",
		keys: [plain(ENTER, { shift: true }), plain(ENTER, { alt: true })],
	},
	{
		what: t("Edit the last queued message, from an empty composer"),
		group: "Composer",
		keys: [plain(UP)],
	},
	{
		what: t("Bring back an earlier message, with the caret at the start"),
		group: "Composer",
		keys: [plain(UP), plain(DOWN)],
	},
	{ what: t("Stop the agent's turn"), group: "Composer", id: "stop" },
	{ what: t("Attach files"), group: "Composer", keys: [keyHint(ATTACH)] },
	{
		what: t("Move the lines up or down, in a note"),
		group: "Notes",
		keys: [plain(UP, { alt: true }), plain(DOWN, { alt: true })],
	},
	{
		what: t("Copy the lines up or down, in a note"),
		group: "Notes",
		keys: [plain(UP, { alt: true, shift: true }), plain(DOWN, { alt: true, shift: true })],
	},
];

const sameKey = (a: Shortcut, b: Shortcut): boolean =>
	a.code === b.code &&
	!!a.shift === !!b.shift &&
	!!a.alt === !!b.alt &&
	!!a.control === !!b.control;

// "nextTab" as "Next tab", for a shortcut with no words yet.
const nameOf = (id: string): string => {
	const spaced = id.replace(/([A-Z])/g, " $1").toLowerCase();
	return spaced.charAt(0).toUpperCase() + spaced.slice(1);
};

/**
 * Every shortcut, by group, each group in the order its lines are written.
 * `pick` is what Enter does while an agent works.
 */
export const shortcutRows = (pick: BusySend = DEFAULT_BUSY_SEND): readonly ShortcutRow[] => {
	const all = Object.entries(SHORTCUTS) as [Id, Shortcut][];
	const used = new Set<Id>();
	const rows: ShortcutRow[] = lines(pick).map((line) => {
		const row = { what: line.what, group: line.group, projectsOnly: !!line.projectsOnly };
		const found =
			"id" in line
				? all.find(([id]) => id === line.id)
				: line.anywhere
					? all.find(([, s]) => sameKey(s, line.anywhere!))
					: undefined;
		if (found) {
			used.add(found[0]);
			return { ...row, keys: [keyHint(found[1])] };
		}
		return { ...row, keys: "keys" in line ? line.keys : [] };
	});
	for (const [id, s] of all) {
		if (!used.has(id)) {
			rows.push({
				what: nameOf(id),
				keys: [keyHint(s)],
				group: "Other",
				projectsOnly: false,
			});
		}
	}
	return SHORTCUT_GROUPS.flatMap((group) =>
		rows.filter((row) => row.group === group && row.keys.length > 0)
	);
};
