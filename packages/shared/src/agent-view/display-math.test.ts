// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { mathSegments } from "./display-math";

describe("display math", () => {
	it("leaves a text with none as one run of Markdown", () => {
		expect(mathSegments("Some *text* and $5.")).toEqual([
			{ kind: "markdown", text: "Some *text* and $5." },
		]);
		expect(mathSegments("$$x$$ and more")).toEqual([
			{ kind: "markdown", text: "$$x$$ and more" },
		]);
	});

	it("cuts a block out between the Markdown around it", () => {
		expect(mathSegments("Before\n$$\nx^2 +\n  y^2\n$$\nAfter")).toEqual([
			{ kind: "markdown", text: "Before" },
			{ kind: "math", tex: "x^2 +\n  y^2", closed: true },
			{ kind: "markdown", text: "After" },
		]);
	});

	it("leaves out runs of Markdown that are only blank", () => {
		expect(mathSegments("\n$$\na\n$$\n\n$$\nb\n$$\n")).toEqual([
			{ kind: "math", tex: "a", closed: true },
			{ kind: "math", tex: "b", closed: true },
		]);
	});

	it("closes on at least as many marks, and drops what follows the opening ones", () => {
		expect(mathSegments("$$$ note\na\n$$\nb\n$$$$  \nc")).toEqual([
			{ kind: "math", tex: "a\n$$\nb", closed: true },
			{ kind: "markdown", text: "c" },
		]);
	});

	it("takes the opening line's indent off each line", () => {
		expect(mathSegments("  $$\n    a\n b\n  $$")).toEqual([
			{ kind: "math", tex: "  a\nb", closed: true },
		]);
	});

	it("runs a block that hasn't closed to the end", () => {
		expect(mathSegments("Text\n\n$$\n\\frac{1}{2}\n")).toEqual([
			{ kind: "markdown", text: "Text\n" },
			{ kind: "math", tex: "\\frac{1}{2}\n", closed: false },
		]);
		expect(mathSegments("$$")).toEqual([{ kind: "math", tex: "", closed: false }]);
	});

	it("cuts out a formula on a line of its own", () => {
		expect(mathSegments("See\n$$ e = mc^2 $$\nthat")).toEqual([
			{ kind: "markdown", text: "See" },
			{ kind: "math", tex: "e = mc^2", closed: true },
			{ kind: "markdown", text: "that" },
		]);
		expect(mathSegments("$$a$$ b $$")).toEqual([{ kind: "markdown", text: "$$a$$ b $$" }]);
		expect(mathSegments("$$a$$$")).toEqual([{ kind: "markdown", text: "$$a$$$" }]);
	});

	it("finds no math inside fenced code", () => {
		const text = "```tex\n$$\nx\n$$\n```\n~~~~\n$$y$$\n~~~\n~~~~\n$$\nz\n$$";
		expect(mathSegments(text)).toEqual([
			{ kind: "markdown", text: "```tex\n$$\nx\n$$\n```\n~~~~\n$$y$$\n~~~\n~~~~" },
			{ kind: "math", tex: "z", closed: true },
		]);
	});

	it("leaves a block that's indented too far or quoted in the text", () => {
		const text = "    $$\n    x\n    $$\n> $$\n> y\n> $$";
		expect(mathSegments(text)).toEqual([{ kind: "markdown", text }]);
	});

	it("reads carriage returns as line breaks", () => {
		expect(mathSegments("a\r\n$$\r\nx\r\n$$\r\nb")).toEqual([
			{ kind: "markdown", text: "a" },
			{ kind: "math", tex: "x", closed: true },
			{ kind: "markdown", text: "b" },
		]);
	});
});
