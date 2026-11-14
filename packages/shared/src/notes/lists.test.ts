// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { applyReplacements } from "./diff";
import { continueList, type ListEdit, listLineOf, shiftList } from "./lists";

/**
 * A text with the selection marked in it: "|" for a caret, or "[" and "]"
 * around a selection, anchor first.
 */
const marked = (text: string) => {
	const caret = text.indexOf("|");
	if (caret !== -1)
		return { text: text.replace("|", ""), selection: { anchor: caret, head: caret } };
	const anchor = text.indexOf("[");
	const head = text.indexOf("]") - 1;
	return { text: text.replace("[", "").replace("]", ""), selection: { anchor, head } };
};

/** The text after an edit, with its selection marked the same way. */
const after = (text: string, edit: ListEdit | null): string | null => {
	if (!edit) return null;
	const next = applyReplacements(text, edit.replacements);
	const { anchor, head } = edit.selection;
	if (anchor === head) return next.slice(0, head) + "|" + next.slice(head);
	return next.slice(0, anchor) + "[" + next.slice(anchor, head) + "]" + next.slice(head);
};

const enter = (input: string) => {
	const { text, selection } = marked(input);
	return after(text, continueList(text, selection));
};

const tab = (input: string, outdent = false) => {
	const { text, selection } = marked(input);
	return after(text, shiftList(text, selection, outdent));
};

describe("listLineOf", () => {
	it("reads bullets, numbers, tasks, quotes and indents", () => {
		expect(listLineOf("- a")).toMatchObject({ marker: "- ", width: 2, contentStart: 2 });
		expect(listLineOf("12. a")).toMatchObject({ marker: "12. ", number: "12", width: 4 });
		expect(listLineOf("1) a")).toMatchObject({ marker: "1) ", width: 3 });
		expect(listLineOf("  * [x] a")).toMatchObject({
			indent: 2,
			task: true,
			width: 2,
			contentStart: 8,
		});
		expect(listLineOf("> + a")).toMatchObject({ quote: "> ", indentEnd: 2, contentStart: 4 });
		expect(listLineOf("\t- a")).toMatchObject({ indent: 4 });
	});

	it("leaves out what only looks like a list", () => {
		for (const line of ["-", "-a", "plain", "- - -", "* * *", "1.a", "1234567890. a", "#"])
			expect(listLineOf(line)).toBeNull();
	});
});

describe("continueList", () => {
	it("starts the next item with the same marker", () => {
		expect(enter("- a|")).toBe("- a\n- |");
		expect(enter("* a|")).toBe("* a\n* |");
		expect(enter("+ a|")).toBe("+ a\n+ |");
	});

	it("counts on an ordered item, keeping its delimiter and zero padding", () => {
		expect(enter("1. a|")).toBe("1. a\n2. |");
		expect(enter("9) a|")).toBe("9) a\n10) |");
		expect(enter("09. a|")).toBe("09. a\n10. |");
	});

	it("starts a task with its box empty", () => {
		expect(enter("- [x] done|")).toBe("- [x] done\n- [ ] |");
		expect(enter("1. [ ] a|")).toBe("1. [ ] a\n2. [ ] |");
	});

	it("keeps the indent and any quote", () => {
		expect(enter("    - a|")).toBe("    - a\n    - |");
		expect(enter("> > 1. a|")).toBe("> > 1. a\n> > 2. |");
		expect(enter(">   - a|")).toBe(">   - a\n>   - |");
	});

	it("moves the text after the caret into the new item", () => {
		expect(enter("- ab|cd\nnext")).toBe("- ab\n- |cd\nnext");
	});

	it("takes out a selection first, even across lines", () => {
		expect(enter("- a[bc]d")).toBe("- a\n- |d");
		expect(enter("- a[b\n- c]d")).toBe("- a\n- |d");
	});

	it("ends the list on an empty item at the left edge", () => {
		expect(enter("- a\n- |")).toBe("- a\n|");
		expect(enter("- a\n- [ ] |")).toBe("- a\n|");
		expect(enter("- a\n3.  |  ")).toBe("- a\n|");
		expect(enter("> - a\n> - |")).toBe("> - a\n> |");
	});

	it("moves an empty nested item out a level instead", () => {
		expect(enter("- a\n  - b\n  - |")).toBe("- a\n  - b\n- |");
		expect(enter("1. a\n   - b\n      - c\n      - |")).toBe("1. a\n   - b\n      - c\n   - |");
	});

	it("does nothing outside a list or with the caret in the marker", () => {
		expect(enter("plain|")).toBeNull();
		expect(enter("|- a")).toBeNull();
		expect(enter("-| a")).toBeNull();
		expect(enter("---|")).toBeNull();
		expect(enter("- a\n|")).toBeNull();
	});
});

describe("shiftList", () => {
	it("nests an item under the one above by that item's width", () => {
		expect(tab("- a\n- b|")).toBe("- a\n  - b|");
		expect(tab("1. a\n2. b|")).toBe("1. a\n   2. b|");
		expect(tab("10. a\n11. |b")).toBe("10. a\n    11. |b");
		expect(tab("- [ ] a\n- [ ] b|")).toBe("- [ ] a\n  - [ ] b|");
	});

	it("goes by the item's own width with nothing above at its level", () => {
		expect(tab("- a|")).toBe("  - a|");
		expect(tab("1. a\n   - b|")).toBe("1. a\n     - b|");
	});

	it("moves a line inside a quote after its markers", () => {
		expect(tab("> - a\n> - b|")).toBe("> - a\n>   - b|");
		expect(tab("> - a\n>   - b|", true)).toBe("> - a\n> - b|");
	});

	it("moves an item out to its parent's level, never below the edge", () => {
		expect(tab("1. a\n   - b|", true)).toBe("1. a\n- b|");
		expect(tab("- a\n    - b|", true)).toBe("- a\n- b|");
		expect(tab("  - a|", true)).toBe("- a|");
		expect(tab("- a|", true)).toBe("- a|");
	});

	it("moves every line of a selection by the same amount", () => {
		expect(tab("- a\n[- b\n  - c\n  more]", false)).toBe("- a\n[  - b\n    - c\n    more]");
		expect(tab("- a\n  [- b\n\n    - c]", true)).toBe("- a\n[- b\n\n  - c]");
		// A selection ending at a line's start leaves that line be.
		expect(tab("- a\n[- b\n]- c")).toBe("- a\n[  - b\n]- c");
	});

	it("turns a tab in the indent into spaces", () => {
		expect(tab("- a\n\t- b|", true)).toBe("- a\n- b|");
		expect(tab("- a\n  - b\n\t- c|", true)).toBe("- a\n  - b\n  - c|");
	});

	it("stops looking up at text that ends the list", () => {
		expect(tab("1. a\nText\n- b|")).toBe("1. a\nText\n  - b|");
	});

	it("does nothing when no line is a list item", () => {
		expect(tab("plain|")).toBeNull();
		expect(tab("[a\nb]")).toBeNull();
	});
});
