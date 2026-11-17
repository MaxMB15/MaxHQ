// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { ItemBody, TimelineItem } from "../bindings";

import { runHistory, walkRuns } from "./history";

const item = (id: string, body: ItemBody): TimelineItem => ({
	id,
	agent: "a1",
	turn: null,
	parent: null,
	ordinal: Number(id.slice(1)),
	status: "completed",
	started_at: null,
	completed_at: null,
	...body,
});

const ran = (id: string, command: string): TimelineItem =>
	item(id, {
		kind: "user_run",
		terminal: `t-${id}`,
		command,
		cwd: "~/code/example-app",
		device: "dev-mac",
		private: false,
		process: { state: "exited", code: 0, signal: null },
		duration_ms: 100,
		lines: 0,
		output: "",
		output_truncated: false,
		moved: false,
	});

const timeline = (items: TimelineItem[]) => ({
	items: new Map(items.map((each) => [each.id, each])),
	order: items.map((each) => each.id),
});

describe("runHistory", () => {
	it("gives the commands you ran, newest first, each once", () => {
		const state = timeline([
			ran("i1", "pnpm test"),
			item("i2", { kind: "assistant_message", text: "pnpm lint", streaming: false }),
			ran("i3", "pnpm lint"),
			ran("i4", "pnpm test "),
			ran("i5", "   "),
		]);
		expect(runHistory(state)).toEqual(["pnpm test", "pnpm lint"]);
	});

	it("is empty with no commands", () => {
		expect(runHistory(timeline([]))).toEqual([]);
	});
});

describe("walkRuns", () => {
	const entries = ["pnpm test", "pnpm lint", "git status"];

	it("walks back from what you typed and forward to it again", () => {
		const first = walkRuns(entries, null, "backward", "pnpm b");
		expect(first).toEqual({ text: "pnpm test", walk: { index: 0, typed: "pnpm b" } });
		const second = walkRuns(entries, first!.walk, "backward", first!.text);
		expect(second).toEqual({ text: "pnpm lint", walk: { index: 1, typed: "pnpm b" } });
		const back = walkRuns(entries, second!.walk, "forward", second!.text);
		expect(back).toEqual({ text: "pnpm test", walk: { index: 0, typed: "pnpm b" } });
		expect(walkRuns(entries, back!.walk, "forward", back!.text)).toEqual({
			text: "pnpm b",
			walk: null,
		});
	});

	it("stops at the oldest, and ↓ does nothing outside a walk", () => {
		expect(walkRuns(entries, { index: 2, typed: "" }, "backward", "git status")).toBeNull();
		expect(walkRuns(entries, null, "forward", "")).toBeNull();
		expect(walkRuns([], null, "backward", "")).toBeNull();
	});
});
