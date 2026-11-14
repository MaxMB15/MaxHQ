// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { applyReplacements } from "./diff";
import type { ListEdit, OffsetSelection } from "./lists";
import { copyLines, type LineDirection, moveLines } from "./move-lines";

/** A text with "|" for a caret, or "[" and "]" around a selection, anchor first. */
const marked = (text: string): { text: string; selection: OffsetSelection } => {
	const caret = text.indexOf("|");
	if (caret !== -1)
		return { text: text.replace("|", ""), selection: { anchor: caret, head: caret } };
	const anchor = text.indexOf("[");
	const head = text.indexOf("]") - 1;
	return { text: text.replace("[", "").replace("]", ""), selection: { anchor, head } };
};

/** The text after an edit, its selection marked the same way. */
const after = (text: string, edit: ListEdit): string => {
	const next = applyReplacements(text, edit.replacements);
	const { anchor, head } = edit.selection;
	if (anchor === head) return next.slice(0, head) + "|" + next.slice(head);
	return next.slice(0, anchor) + "[" + next.slice(anchor, head) + "]" + next.slice(head);
};

const move = (input: string, direction: LineDirection) => {
	const { text, selection } = marked(input);
	return after(text, moveLines(text, selection, direction));
};

const copy = (input: string, direction: LineDirection) => {
	const { text, selection } = marked(input);
	return after(text, copyLines(text, selection, direction));
};

describe("moveLines", () => {
	it("moves the caret's line up and down, the caret with it", () => {
		expect(move("one\ntw|o\nthree", -1)).toBe("tw|o\none\nthree");
		expect(move("one\ntw|o\nthree", 1)).toBe("one\nthree\ntw|o");
	});

	it("moves every line the selection touches, keeping the selection", () => {
		expect(move("a\nb[b\nc]c\nd", -1)).toBe("b[b\nc]c\na\nd");
		expect(move("a\nb[b\nc]c\nd", 1)).toBe("a\nd\nb[b\nc]c");
	});

	it("keeps a selection that runs backwards", () => {
		const edit = moveLines("a\nbb\ncc", { anchor: 7, head: 3 }, -1);
		expect(applyReplacements("a\nbb\ncc", edit.replacements)).toBe("bb\ncc\na");
		expect(edit.selection).toEqual({ anchor: 5, head: 1 });
	});

	it("leaves out the line a selection only reaches the start of", () => {
		expect(move("a\n[bb\n]cc", 1)).toBe("a\ncc\n[bb]");
	});

	it("does nothing at the first line going up or the last going down", () => {
		const top = moveLines("o|ne\ntwo", { anchor: 1, head: 1 }, -1);
		expect(top.replacements).toEqual([]);
		expect(top.selection).toEqual({ anchor: 1, head: 1 });
		expect(moveLines("one\ntwo", { anchor: 5, head: 5 }, 1).replacements).toEqual([]);
		expect(move("[one\ntwo]", 1)).toBe("[one\ntwo]");
	});

	it("moves blank lines, list items and lines in a code block as they are", () => {
		expect(move("\n|x", -1)).toBe("|x\n");
		expect(move("- one\n  - tw|o", -1)).toBe("  - tw|o\n- one");
		expect(move("```\nlet a|\nlet b\n```", 1)).toBe("```\nlet b\nlet a|\n```");
		expect(move("a\n|", -1)).toBe("|\na");
	});
});

describe("copyLines", () => {
	it("copies the lines down, the selection on the lower copy", () => {
		expect(copy("one\ntw|o\nthree", 1)).toBe("one\ntwo\ntw|o\nthree");
		expect(copy("[a\nb]", 1)).toBe("a\nb\n[a\nb]");
	});

	it("copies the lines up, the selection on the upper copy", () => {
		expect(copy("one\ntw|o", -1)).toBe("one\ntw|o\ntwo");
		expect(copy("t[w]o", -1)).toBe("t[w]o\ntwo");
	});

	it("copies at the first and last lines too", () => {
		expect(copy("o|ne", -1)).toBe("o|ne\none");
		expect(copy("o|ne", 1)).toBe("one\no|ne");
	});
});
