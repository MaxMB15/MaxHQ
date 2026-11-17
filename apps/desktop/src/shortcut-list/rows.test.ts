// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it, vi } from "vitest";

import { keyHint, SHORTCUTS, type Shortcut } from "@/layout/shortcuts";

import { SHORTCUT_GROUPS, shortcutRows } from "./rows";

const onMac = () =>
	vi
		.spyOn(navigator, "userAgent", "get")
		.mockReturnValue("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)");

const onWindows = () =>
	vi
		.spyOn(navigator, "userAgent", "get")
		.mockReturnValue("Mozilla/5.0 (Windows NT 10.0; Win64; x64)");

const rowOf = (what: string) => shortcutRows().find((row) => row.what === what);

// Adds shortcuts to `SHORTCUTS` for one test, as another change would.
const withMore = (more: Record<string, Shortcut>, run: () => void) => {
	Object.assign(SHORTCUTS, more);
	try {
		run();
	} finally {
		for (const id of Object.keys(more)) Reflect.deleteProperty(SHORTCUTS, id);
	}
};

afterEach(() => {
	vi.restoreAllMocks();
});

describe("the shortcut list", () => {
	it("lists every shortcut in SHORTCUTS once, as menus write it", () => {
		onMac();
		const rows = shortcutRows();
		for (const s of Object.values(SHORTCUTS)) {
			expect(rows.filter((row) => row.keys.includes(keyHint(s)))).toHaveLength(1);
		}
		expect(rowOf("Split down")).toEqual({
			what: "Split down",
			keys: ["⇧⌘D"],
			group: "Panes",
			projectsOnly: true,
		});
		expect(rowOf("Show keyboard shortcuts")?.keys).toEqual(["⌘/"]);
		expect(rowOf("Show or hide the terminal")).toMatchObject({ keys: ["⌃`"], group: "Panes" });
		expect(rowOf("Stop the agent's turn")).toMatchObject({ keys: ["⌘."], group: "Composer" });
	});

	it("says what Enter and Command+Enter do as this Mac picked for a working agent", () => {
		onMac();
		const sending = (pick?: "queue" | "steer") =>
			shortcutRows(pick)
				.filter((row) => row.group === "Composer")
				.filter((row) => ["⏎", "⌘⏎"].includes(row.keys.join()))
				.map((row) => [row.keys.join(), row.what]);
		expect(sending()).toEqual([
			["⏎", "Send, or queue while the agent works"],
			["⌘⏎", "Send at once to steer the agent"],
		]);
		expect(sending("queue")).toEqual(sending());
		expect(sending("steer")).toEqual([
			["⏎", "Send, or steer while the agent works"],
			["⌘⏎", "Queue until the agent finishes"],
		]);
	});

	it("adds the keys other parts of the app listen for", () => {
		onMac();
		expect(rowOf("New agent")).toMatchObject({ keys: ["⌘N"], group: "Agents" });
		expect(rowOf("Open settings")?.keys).toEqual(["⌘,"]);
		expect(rowOf("Attach files")?.keys).toEqual(["⌘U"]);
		expect(rowOf("Send at once to steer the agent")?.keys).toEqual(["⌘⏎"]);
		expect(rowOf("New line")?.keys).toEqual(["⇧⏎", "⌥⏎"]);
		expect(rowOf("Bring back an earlier message, with the caret at the start")).toMatchObject({
			keys: ["↑", "↓"],
			group: "Composer",
		});
		expect(rowOf("Move the lines up or down, in a note")).toMatchObject({
			keys: ["⌥↑", "⌥↓"],
			group: "Notes",
		});
		expect(rowOf("Copy the lines up or down, in a note")?.keys).toEqual(["⌥⇧↑", "⌥⇧↓"]);
		expect(rowOf("Pick that option, on a question card with focus")?.keys).toEqual(["1–9"]);
		expect(rowOf("Move to the pane on that side")).toMatchObject({
			keys: ["⌥⌘←→↑↓"],
			projectsOnly: true,
		});
	});

	it("lists the sidebar's keys, which work in either mode", () => {
		onMac();
		expect(rowOf("Show or hide the sidebar")).toEqual({
			what: "Show or hide the sidebar",
			keys: ["⌘B"],
			group: "Sidebar",
			projectsOnly: false,
		});
		expect(rowOf("Filter the agents")).toMatchObject({ keys: ["⌥⌘F"], group: "Sidebar" });
		expect(rowOf("Open the first agent the filter shows")?.keys).toEqual(["⏎"]);
		expect(rowOf("Clear the filter, then leave it")?.keys).toEqual(["Esc"]);
		onWindows();
		expect(rowOf("Show or hide the sidebar")?.keys).toEqual(["Ctrl+B"]);
		expect(rowOf("Filter the agents")?.keys).toEqual(["Ctrl+Alt+F"]);
		expect(rowOf("Resize the sidebar, on its edge with focus")?.keys).toEqual(["Left/Right"]);
		expect(rowOf("Move between the agents, on an agent with focus")?.keys).toEqual(["Up/Down"]);
	});

	it("marks the keys that work only in projects mode", () => {
		onMac();
		expect(rowOf("Find in the conversation")?.projectsOnly).toBe(true);
		expect(rowOf("Next tab")?.projectsOnly).toBe(true);
		// These open a pane or need none, so they work in devices mode too.
		expect(rowOf("Open the next agent waiting on you")?.projectsOnly).toBe(false);
		expect(rowOf("Reopen the last closed tab")?.projectsOnly).toBe(false);
		expect(
			rowOf("Settle and go to the next agent waiting, or settle the picked ones")
				?.projectsOnly
		).toBe(false);
	});

	it("spells the keys out off a Mac", () => {
		onWindows();
		expect(rowOf("Split down")?.keys).toEqual(["Ctrl+Shift+D"]);
		expect(rowOf("Start the agent, in the New agent form")?.keys).toEqual(["Ctrl+Enter"]);
		expect(rowOf("New line")?.keys).toEqual(["Shift+Enter", "Alt+Enter"]);
		expect(rowOf("Bring back an earlier message, with the caret at the start")?.keys).toEqual([
			"Up",
			"Down",
		]);
		expect(rowOf("Show the tab at that place, 9 for the last")?.keys).toEqual(["Ctrl+1–9"]);
	});

	it("keeps the groups in order", () => {
		const order = shortcutRows().map((row) => SHORTCUT_GROUPS.indexOf(row.group));
		expect(order).toEqual([...order].sort((a, b) => a - b));
	});

	it("shows a new shortcut under Other until it has words", () => {
		onMac();
		withMore({ focusSidebar: { label: "E", code: "KeyE", shift: true } }, () => {
			expect(rowOf("Focus sidebar")).toEqual({
				what: "Focus sidebar",
				keys: ["⇧⌘E"],
				group: "Other",
				projectsOnly: false,
			});
		});
	});

	it("shows a key written out here once when SHORTCUTS has it too", () => {
		onMac();
		expect(SHORTCUTS).toHaveProperty("newAgent");
		const rows = shortcutRows().filter((row) => row.keys.includes("⌘N"));
		expect(rows).toEqual([
			{ what: "New agent", keys: ["⌘N"], group: "Agents", projectsOnly: false },
		]);
	});
});
