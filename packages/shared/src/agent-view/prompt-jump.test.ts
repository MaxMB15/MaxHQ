// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { TimelineItem } from "../bindings";
import { reply } from "../test/timeline";
import { timelineItem } from "../timeline/build";
import {
	itemLine,
	placeKey,
	promptAt,
	promptBefore,
	promptLine,
	promptPlace,
	promptPreview,
	promptRows,
	promptTotal,
	stepPreview,
	stepPrompt,
} from "./prompt-jump";

const you = (id: string, ordinal: number, text = id): TimelineItem =>
	timelineItem(
		{ kind: "user_message", text, attachments: [], delivery: "sent" },
		{ id, ordinal }
	);

const marks = [{ top: 100 }, { top: 900 }, { top: 2000 }];
const at = (top: number, more: Partial<{ older: boolean; atEnd: boolean }> = {}) => ({
	top,
	older: false,
	atEnd: false,
	...more,
});

describe("stepPrompt", () => {
	it("steps to the prompts either side of the view's top", () => {
		expect(stepPrompt(marks, at(1200), -1)).toEqual({ kind: "prompt", index: 1 });
		expect(stepPrompt(marks, at(1200), 1)).toEqual({ kind: "prompt", index: 2 });
	});

	it("leaves the prompt at the top for the ones around it", () => {
		expect(stepPrompt(marks, at(900), -1)).toEqual({ kind: "prompt", index: 0 });
		expect(stepPrompt(marks, at(900), 1)).toEqual({ kind: "prompt", index: 2 });
		expect(stepPrompt(marks, at(904), 1)).toEqual({ kind: "prompt", index: 2 });
	});

	it("has nothing earlier at the very top, unless older pages wait there", () => {
		expect(stepPrompt(marks, at(50), -1)).toBeNull();
		expect(stepPrompt(marks, at(50, { older: true }), -1)).toEqual({ kind: "older" });
		expect(stepPrompt(marks, at(0, { older: true }), -1)).toEqual({ kind: "older" });
		expect(stepPrompt([], at(0, { older: true }), -1)).toEqual({ kind: "older" });
	});

	it("goes to the latest message past the last prompt, and nowhere at the end", () => {
		expect(stepPrompt(marks, at(2500), 1)).toEqual({ kind: "latest" });
		expect(stepPrompt(marks, at(1200, { atEnd: true }), 1)).toBeNull();
	});
});

describe("the prompts loaded", () => {
	const items = [you("p1", 1), reply("r1", 2), you("p2", 3), reply("r2", 4)];
	const state = {
		rows: items.map((item) => item.id),
		items: new Map(items.map((item) => [item.id, item])),
	};

	it("finds them among the rows, by row", () => {
		expect(promptRows(state.rows, state.items)).toEqual([
			{ id: "p1", row: 0 },
			{ id: "p2", row: 2 },
		]);
	});

	it("finds the last one before a place, or none", () => {
		expect(promptBefore(state, 4)).toBe("p2");
		expect(promptBefore(state, 3)).toBe("p1");
		expect(promptBefore(state, 1)).toBeNull();
		expect(promptBefore(state, Number.POSITIVE_INFINITY)).toBe("p2");
	});

	it("counts them only once no older page is left", () => {
		expect(promptTotal(2, false)).toBe(2);
		expect(promptTotal(2, true)).toBeNull();
	});

	it("reads a prompt's line again when its words change", () => {
		const first = you("p1", 1, "Fix the header\nand more");
		expect(itemLine(first)).toBe("Fix the header");
		expect(itemLine(you("p1", 1, "Fix the footer"))).toBe("Fix the footer");
		expect(itemLine(reply("r1", 2))).toBe("");
		expect(itemLine(undefined)).toBe("");
	});
});

describe("promptAt", () => {
	it("is the last prompt at or above the view's top, else the first", () => {
		expect(promptAt(marks, 0)).toBe(0);
		expect(promptAt(marks, 896)).toBe(1);
		expect(promptAt(marks, 1999)).toBe(2);
	});
});

describe("placeKey", () => {
	it("changes only when the place does", () => {
		expect(placeKey(promptPlace(marks, at(1000)))).toBe(placeKey(promptPlace(marks, at(1500))));
		expect(placeKey(promptPlace(marks, at(1000)))).not.toBe(
			placeKey(promptPlace(marks, at(2100)))
		);
	});
});

describe("previews", () => {
	it("takes a prompt's first line, shortened", () => {
		expect(promptLine("  Fix the header\nand the rest")).toBe("Fix the header");
		expect(promptLine("abcdefghij", 6)).toBe("abcde…");
	});

	it("says where a step goes", () => {
		const lineOf = (index: number) => ["One", "Two", "Three"][index]!;
		expect(stepPreview({ kind: "prompt", index: 1 }, 3, lineOf)).toBe("2 of 3 · Two");
		expect(stepPreview({ kind: "latest" }, 3, lineOf)).toBe("Latest message");
		expect(stepPreview({ kind: "older" }, 3, lineOf)).toBe("Earlier prompt");
		expect(stepPreview(null, 3, lineOf)).toBeNull();
	});

	it("leaves out a total it doesn't know", () => {
		expect(promptPreview(1, null, "Two")).toBe("Two");
		expect(stepPreview({ kind: "prompt", index: 1 }, null, () => "Two")).toBe("Two");
	});
});
