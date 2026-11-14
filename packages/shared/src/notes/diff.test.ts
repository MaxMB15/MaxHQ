// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { applyReplacements, replacementsBetween } from "./diff";

/** Checks the replacements rebuild `after`, and gives them back. */
const between = (before: string, after: string) => {
	const replacements = replacementsBetween(before, after);
	expect(applyReplacements(before, replacements)).toBe(after);
	return replacements;
};

describe("replacementsBetween", () => {
	it("finds nothing between equal texts", () => {
		expect(between("a\nb\n", "a\nb\n")).toEqual([]);
	});

	it("replaces only the characters that changed inside a line", () => {
		expect(between("one\nthe cat sat\nthree", "one\nthe dog sat\nthree")).toEqual([
			{ from: 8, to: 11, text: "dog" },
		]);
	});

	it("adds and removes whole lines", () => {
		expect(between("a\nc\n", "a\nb\nc\n")).toEqual([{ from: 2, to: 2, text: "b\n" }]);
		expect(between("a\nb\nc\n", "a\nc\n")).toEqual([{ from: 2, to: 4, text: "" }]);
	});

	it("handles a last line with no line break, and empty texts", () => {
		between("a\nb", "a\nb\nc");
		between("a\nb\nc", "a\nb");
		between("", "x\ny\n");
		between("x\ny\n", "");
		between("\n\n\n", "\n");
	});

	it("keeps the replacements in order and apart", () => {
		const before = Array.from({ length: 50 }, (_, i) => `line ${i}`).join("\n");
		const after = before.replace("line 3", "line three").replace("line 40", "").concat("\nend");
		const replacements = between(before, after);
		for (let i = 1; i < replacements.length; i++)
			expect(replacements[i]!.from).toBeGreaterThanOrEqual(replacements[i - 1]!.to);
	});

	it("puts in a long run of changed lines whole", () => {
		const before = `start\n${"a".repeat(15_000)}\nend`;
		const after = `start\n${"b".repeat(15_000)}\nend`;
		expect(between(before, after)).toHaveLength(1);
	});
});
