// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { TimelineItem } from "@maxhq/shared/bindings";
import { reply } from "@maxhq/shared/test/timeline";
import { timelineItem } from "@maxhq/shared/timeline/build";
import type { FeedRow } from "@maxhq/shared/timeline/output";

import { promptAtTop, promptEntries } from "./prompts";

const you = (id: string, ordinal: number, text = id): TimelineItem =>
	timelineItem(
		{ kind: "user_message", text, attachments: [], delivery: "sent" },
		{ id, ordinal }
	);

const feed = [
	you("p1", 1, "Fix the header\nand the footer"),
	reply("r1", 2),
	you("p2", 3, "Run the tests"),
	reply("r2", 4),
	reply("r3", 5),
	you("p3", 6, "Ship it"),
	reply("r4", 7),
];
const items = new Map(feed.map((item) => [item.id, item]));
const rows: FeedRow[] = feed.map((item) => ({
	id: item.id,
	kind: item.kind,
	gap: 0,
	attention: null,
}));

describe("your prompts", () => {
	it("lists them newest first, numbered once all are loaded", () => {
		expect(promptEntries(rows, items, false)).toEqual([
			{ id: "p3", row: 5, number: 3, line: "Ship it" },
			{ id: "p2", row: 2, number: 2, line: "Run the tests" },
			{ id: "p1", row: 0, number: 1, line: "Fix the header" },
		]);
	});

	it("leaves the numbers out while older pages may hold more", () => {
		expect(promptEntries(rows, items, true).map((entry) => entry.number)).toEqual([
			null,
			null,
			null,
		]);
	});

	it("picks the last prompt at or above the top row", () => {
		const entries = promptEntries(rows, items, false);
		expect(promptAtTop(entries, rows, "r3")).toBe("p2");
		expect(promptAtTop(entries, rows, "p2")).toBe("p2");
		expect(promptAtTop(entries, rows, "r4")).toBe("p3");
	});

	it("picks the first prompt above them all, and the newest before the feed says", () => {
		const late = rows.slice(1);
		const entries = promptEntries(late, items, true);
		expect(promptAtTop(entries, late, "r1")).toBe("p2");
		expect(promptAtTop(entries, late, undefined)).toBe("p3");
		expect(promptAtTop(entries, late, "gone")).toBe("p3");
		expect(promptAtTop([], rows, "r1")).toBeNull();
	});
});
