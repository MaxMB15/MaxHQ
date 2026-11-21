// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { EMPTY_TIMELINE, type TimelineState } from "./apply";
import { timelineItem } from "./build";
import {
	cardChanges,
	type FeedCard,
	feedAway,
	feedIndexOf,
	feedRowFor,
	feedRows,
	recentOutput,
	rowsSince,
} from "./output";
import type { TimelineItem } from "../bindings";

const stateOf = (items: TimelineItem[]): TimelineState => ({
	...EMPTY_TIMELINE,
	status: "ready",
	items: new Map(items.map((item) => [item.id, item])),
	order: items.map((item) => item.id),
	rows: items.map((item) => item.id),
});

const items = [
	timelineItem(
		{ kind: "user_message", text: "Fix the meter", attachments: [], delivery: "sent" },
		{ id: "u", ordinal: 0 }
	),
	timelineItem(
		{ kind: "reasoning", text: "Thinking", streaming: false },
		{ id: "r", ordinal: 1 }
	),
	timelineItem(
		{
			kind: "command",
			command: "pnpm test\n--watch",
			cwd: null,
			exit_code: 0,
			output: "",
			output_truncated: false,
			background: false,
		},
		{ id: "c", ordinal: 2 }
	),
	timelineItem(
		{ kind: "file_read", path: "src/meter.tsx", lines: null },
		{ id: "inner", ordinal: 3, parent: "c" }
	),
	timelineItem(
		{ kind: "assistant_message", text: "Done.", streaming: false },
		{ id: "a", ordinal: 4 }
	),
];

describe("recentOutput", () => {
	it("reads each top-level item as one line, leaving out reasoning", () => {
		expect(recentOutput(stateOf(items)).map((l) => [l.speaker, l.text])).toEqual([
			["you", "Fix the meter"],
			["tool", "Ran pnpm test"],
			["agent", "Done."],
		]);
	});

	it("keeps the newest lines when there are more than it shows", () => {
		expect(recentOutput(stateOf(items), 2).map((l) => l.id)).toEqual(["c", "a"]);
	});

	it("reads files the agent sent as its message, or how many there were", () => {
		const sent = (text: string, count: number) =>
			timelineItem(
				{
					kind: "agent_attachments",
					text,
					attachments: Array.from({ length: count }, (_, i) => ({
						id: `f${i}`,
						name: `f${i}.png`,
						kind: "image" as const,
						bytes: 1,
						mime: null,
						path: `/home/user/f${i}.png`,
						thumbnail: null,
					})),
				},
				{ id: "s" }
			);
		const lineOf = (item: TimelineItem) => recentOutput(stateOf([item]))[0]?.text;
		expect(lineOf(sent("", 1))).toBe("Sent you a file");
		expect(lineOf(sent("", 2))).toBe("Sent you 2 files");
		expect(lineOf(sent("Here you go", 2))).toBe("Here you go");
	});

	it("reads a claude.ai page call as its verb, page and link", () => {
		const published = timelineItem(
			{
				kind: "tool_call",
				server: null,
				name: "Artifact",
				title: "Artifact publish",
				input: null,
				output: null,
				artifact: {
					action: "publish",
					title: "Weekly report",
					url: "https://claude.ai/artifact/ExampleArtifactId0000001",
				},
			},
			{ id: "p" }
		);
		expect(recentOutput(stateOf([published]))[0]?.text).toBe(
			"Published Weekly report https://claude.ai/artifact/ExampleArtifactId0000001"
		);
	});

	it("reads a visual as what it shows", () => {
		const shown = timelineItem(
			{
				kind: "agent_visual",
				title: "Weekly usage",
				attachment: {
					id: "v",
					name: "visual-weekly-usage.html",
					kind: "file",
					bytes: 1,
					mime: "text/html",
					path: "/home/user/visual-weekly-usage.html",
					thumbnail: null,
				},
				replaces: null,
			},
			{ id: "v" }
		);
		expect(recentOutput(stateOf([shown]))).toEqual([
			{ id: "v", speaker: "tool", text: "Showed Weekly usage" },
		]);
	});

	it("names both vendors where the agent moved", () => {
		const moved = (status: TimelineItem["status"], to_model: string | null) =>
			timelineItem(
				{
					kind: "handoff",
					from_vendor: "claude",
					from_model: "opus",
					to_vendor: "codex",
					to_model,
				},
				{ id: "h", status }
			);
		const lineOf = (item: TimelineItem) => recentOutput(stateOf([item]))[0];
		expect(lineOf(moved("pending", "gpt-5.5"))).toEqual({
			id: "h",
			speaker: "note",
			text: "Context handoff pending, Claude opus to Codex gpt-5.5",
		});
		expect(lineOf(moved("completed", null))?.text).toBe(
			"Context handoff, Claude opus to Codex"
		);
		expect(lineOf(moved("cancelled", null))?.text).toBe(
			"Context handoff skipped, Claude opus to Codex"
		);
	});
});

describe("feedRows", () => {
	const approval = timelineItem(
		{
			kind: "approval",
			attention: "e1",
			approval_kind: "command",
			title: "Run pnpm build",
			detail: null,
			cwd: null,
			options: [],
			decided: null,
		},
		{ id: "p", ordinal: 5, status: "waiting" }
	);
	const failed = timelineItem(
		{
			kind: "command",
			command: "pnpm lint",
			cwd: null,
			exit_code: 1,
			output: "1 error\n",
			output_truncated: true,
			background: false,
		},
		{ id: "f", ordinal: 6 }
	);

	it("lists every item the timeline shows, oldest first, with its kind", () => {
		const rows = feedRows(stateOf([...items, approval, failed]));
		expect(rows.map((r) => [r.id, r.kind])).toEqual([
			["u", "user_message"],
			["r", "reasoning"],
			["c", "command"],
			["inner", "file_read"],
			["a", "assistant_message"],
			["p", "approval"],
			["f", "command"],
		]);
	});

	it("gives each row the space above it, from the kind before it", () => {
		const rows = feedRows(stateOf([...items, approval, failed]));
		expect(rows.map((r) => r.gap)).toEqual([0, 14, 2, 1, 10, 10, 10]);
	});

	it("keeps only the items a filter keeps, spaced as they then sit", () => {
		const said = (item: TimelineItem) =>
			item.kind === "user_message" || item.kind === "assistant_message";
		const rows = feedRows(stateOf([...items, approval, failed]), said);
		expect(rows.map((r) => [r.id, r.gap])).toEqual([
			["u", 0],
			["a", 14],
		]);
	});

	it("names the row an item shows in, while the timeline shows it", () => {
		const state = stateOf(items);
		expect(feedRowFor(state, "c")).toBe("c");
		expect(feedRowFor(state, "inner")).toBe("inner");
		expect(feedRowFor(state, "gone")).toBeNull();
	});

	it("finds the card for an attention event", () => {
		const rows = feedRows(stateOf([...items, approval]));
		expect(feedIndexOf(rows, "e1")).toBe(5);
		expect(feedIndexOf(rows, "e2")).toBe(-1);
		expect(rows[5]).toMatchObject({ id: "p", attention: "e1" });
		expect(rows[0]).toMatchObject({ attention: null });
	});

	it("keeps approvals that hang under a tool call", () => {
		const asked = timelineItem(
			{
				kind: "approval",
				attention: "e1",
				approval_kind: "command",
				title: "Run pnpm build",
				detail: "rm -rf build\npnpm build",
				cwd: null,
				options: [],
				decided: null,
			},
			{ id: "p", ordinal: 5, parent: "c", status: "waiting" }
		);
		const rows = feedRows(stateOf([...items, asked]));
		expect(rows.at(-1)).toMatchObject({ id: "p", attention: "e1" });
	});
});

describe("cardChanges", () => {
	const changes = [
		{
			path: "src/meter.tsx",
			old_path: null,
			op: "modify" as const,
			additions: 1,
			deletions: 1,
			patch: "-a\n+b",
		},
	];
	const edit = timelineItem({ kind: "file_change", changes }, { id: "e", ordinal: 5 });
	const card = (parent: string | null) =>
		timelineItem(
			{
				kind: "approval",
				attention: "e1",
				approval_kind: "file_change",
				title: "Edit src/meter.tsx?",
				detail: null,
				cwd: null,
				options: [],
				decided: null,
			},
			{ id: "p", ordinal: 6, parent, status: "waiting" }
		) as FeedCard;

	it("gives a card under a file change the edit it asks about", () => {
		expect(cardChanges(stateOf([...items, edit]), card("e"))).toBe(changes);
	});

	it("gives any other card none", () => {
		expect(cardChanges(stateOf([...items, edit]), card("c"))).toBeNull();
		expect(cardChanges(stateOf(items), card(null))).toBeNull();
		expect(cardChanges(stateOf(items), card("gone"))).toBeNull();
	});
});

describe("feedAway", () => {
	const rows = feedRows(stateOf(items));
	const more = feedRows(
		stateOf([
			...items,
			timelineItem(
				{ kind: "assistant_message", text: "And tested.", streaming: false },
				{ id: "b", ordinal: 5 }
			),
		])
	);

	it("keeps the last row from when you left the end, until you're back", () => {
		const away = feedAway(null, rows, false);
		expect(away).toEqual({ last: "a" });
		expect(feedAway(away, more, false)).toBe(away);
		expect(feedAway(away, more, true)).toBeNull();
	});

	it("says rows came in only once one lands after that row", () => {
		const away = feedAway(null, rows, false);
		expect(rowsSince(away, rows)).toBe(false);
		expect(rowsSince(away, more)).toBe(true);
		expect(rowsSince(null, more)).toBe(false);
		expect(rowsSince(feedAway(null, [], false), rows)).toBe(true);
	});
});
