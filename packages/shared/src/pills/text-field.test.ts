// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { AgentId, DeviceId } from "../bindings";

import type { PlacedPill, Segment } from "./draft";
import {
	editField,
	fieldOf,
	fieldSpans,
	fieldTrigger,
	markedOf,
	replaceMarked,
	selectionBefore,
	shownOf,
	splitMarked,
	tokensOf,
} from "./text-field";

const text = (value: string): Segment => ({ type: "text", text: value });
const file = (key: string): Segment => ({ type: "pill", pill: { type: "attachment", key } });
const agent = (title: string): Segment => ({
	type: "pill",
	pill: {
		type: "agent",
		agent: "a1" as AgentId,
		title,
		vendor: "claude",
		device: "d1" as DeviceId,
		excerpt: null,
	},
});

const facts = new Map([
	["k1", { name: "pasted-image.png", kind: "image" as const }],
	["k2", { name: "main.rs", kind: "file" as const }],
]);
const factsOf = (key: string) => facts.get(key);
const tokenOf = (segments: readonly Segment[]) => tokensOf(segments, factsOf);

describe("tokensOf", () => {
	it("shows a pasted picture by its number, a file by its name and an agent with @", () => {
		const segments = [file("k1"), file("k2"), agent("Usage meter")];
		const tokens = tokenOf(segments);
		const pills = segments.map((segment) => (segment as { pill: PlacedPill }).pill);
		expect(pills.map(tokens)).toEqual(["Image 1", "main.rs", "@Usage meter"]);
	});
});

describe("fieldOf", () => {
	it("puts each pill's token in the text and notes where it sits", () => {
		const segments = [text("see "), file("k2"), text(" now")];
		const field = fieldOf(segments, tokenOf(segments));
		expect(field.text).toBe("see main.rs now");
		expect(field.pieces.map(({ from, to, at }) => [from, to, at])).toEqual([[4, 11, 4]]);
	});
});

describe("offsets", () => {
	const segments = [text("ab"), file("k2"), text("cd")];
	const field = fieldOf(segments, tokenOf(segments));

	it("maps shown offsets to marked ones and back", () => {
		expect(markedOf(field, 2)).toBe(2);
		expect(markedOf(field, 9)).toBe(3);
		expect(markedOf(field, 10)).toBe(4);
		expect(shownOf(field, 3)).toBe(9);
		expect(shownOf(field, 4)).toBe(10);
	});

	it("rounds an offset inside a token to its start, or its end", () => {
		expect(markedOf(field, 5)).toBe(2);
		expect(markedOf(field, 5, true)).toBe(3);
	});
});

describe("splitMarked", () => {
	it("cuts text at the offset and keeps pills whole", () => {
		expect(splitMarked([text("abc"), file("k1"), text("de")], 4)).toEqual([
			[text("abc"), file("k1")],
			[text("de")],
		]);
		expect(splitMarked([text("abc")], 1)).toEqual([[text("a")], [text("bc")]]);
	});
});

describe("replaceMarked", () => {
	it("puts a pill in place of the menu's word, with spaces", () => {
		const edit = replaceMarked([text("look at@age")], 7, 11, [agent("Usage")]);
		expect(edit.segments).toEqual([text("look at "), agent("Usage"), text(" ")]);
		expect(edit.caret).toBe(10);
	});

	it("adds a space after a picked number when asked", () => {
		const edit = replaceMarked([text("fix #4")], 4, 6, [text("#412")], true);
		expect(edit.segments).toEqual([text("fix #412 ")]);
		expect(edit.caret).toBe(9);
	});

	it("steps over a space that's already there", () => {
		const edit = replaceMarked([text("@a here")], 0, 2, [file("k2")]);
		expect(edit.segments).toEqual([file("k2"), text(" here")]);
		expect(edit.caret).toBe(2);
	});
});

describe("editField", () => {
	const segments = [text("see "), file("k2"), text(" now")];
	const tokens = tokenOf(segments);

	it("types plain text where the caret is", () => {
		const change = editField(segments, tokens, "see main.rs now!", { start: 15, end: 15 });
		expect(change.segments).toEqual([text("see "), file("k2"), text(" now!")]);
		expect(change.caret).toBe(16);
		expect(change.same).toBe(true);
	});

	it("types after a token when the caret was inside it", () => {
		const change = editField(segments, tokens, "see maxin.rs now", { start: 6, end: 6 });
		expect(change.segments).toEqual([text("see "), file("k2"), text("x now")]);
		expect(change.caret).toBe(12);
		expect(change.same).toBe(false);
	});

	it("takes a whole pill when a backspace touches it", () => {
		const change = editField(segments, tokens, "see main.r now", { start: 11, end: 11 });
		expect(change.segments).toEqual([text("see  now")]);
		expect(change.caret).toBe(4);
	});

	it("takes a whole pill when a selection covers part of it", () => {
		const change = editField(segments, tokens, "sex now", { start: 2, end: 8 });
		expect(change.segments).toEqual([text("sex now")]);
	});

	it("tells repeated letters apart by the selection", () => {
		const twice = [text("aa"), file("k2")];
		const change = editField(twice, tokenOf(twice), "aaamain.rs", { start: 0, end: 0 });
		expect(change.segments).toEqual([text("aaa"), file("k2")]);
		expect(change.caret).toBe(1);
	});

	it("finds the change from the text alone when the selection doesn't fit", () => {
		const change = editField(segments, tokens, "see main.rs today", null);
		expect(change.segments).toEqual([text("see "), file("k2"), text(" today")]);
	});
});

describe("selectionBefore", () => {
	it("skips a selection that already moved past the edit", () => {
		const moved = { start: 3, end: 3 };
		const before = { start: 2, end: 2 };
		expect(selectionBefore("aa", "aaa", [moved, before])).toEqual(before);
	});

	it("takes the first that fits", () => {
		const first = { start: 0, end: 0 };
		expect(selectionBefore("aa", "aaa", [first, { start: 2, end: 2 }])).toEqual(first);
	});

	it("finds a backspace from its caret", () => {
		expect(selectionBefore("abc", "ac", [null, { start: 2, end: 2 }])).toEqual({
			start: 2,
			end: 2,
		});
	});

	it("gives null when none fits", () => {
		expect(selectionBefore("abc", "xyz", [{ start: 0, end: 0 }])).toBeNull();
	});
});

describe("fieldTrigger", () => {
	it("opens a menu for the word at the caret, counting pills as one mark", () => {
		const segments = [file("k2"), text(" @src")];
		const trigger = fieldTrigger(segments, tokenOf(segments), 12);
		expect(trigger).toEqual({ kind: "path", query: "src", rangeStart: 2, rangeEnd: 6 });
	});

	it("opens none with the caret inside a token", () => {
		const segments = [agent("Usage"), text(" ")];
		expect(fieldTrigger(segments, tokenOf(segments), 3)).toBeNull();
	});
});

describe("fieldSpans", () => {
	it("splits the text into plain runs, pills and shown numbers", () => {
		const segments = [text("fix #12 in "), file("k2")];
		const spans = fieldSpans(segments, tokenOf(segments), (key) => key === "#12");
		expect(spans.map((span) => [span.kind, span.text])).toEqual([
			["text", "fix "],
			["git", "#12"],
			["text", " in "],
			["pill", "main.rs"],
		]);
	});
});
