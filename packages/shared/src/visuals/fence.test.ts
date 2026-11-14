// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { fromVisualFence, toVisualFence, visualFence, visualsIn } from "./fence";

describe("visual fences", () => {
	it("reads the title from the info string", () => {
		expect(visualFence("html", 'visual title="Token use by day"')).toEqual({
			title: "Token use by day",
		});
		expect(visualFence("HTML", "visual")).toEqual({ title: "" });
		expect(visualFence("html", 'visual  height=3 title="A \\"b\\" \\\\ c"')).toEqual({
			title: 'A "b" \\ c',
		});
	});

	it("leaves plain html and other languages as code", () => {
		expect(visualFence("html", null)).toBeNull();
		expect(visualFence("html", "")).toBeNull();
		expect(visualFence("html", "visuals")).toBeNull();
		expect(visualFence("html", 'title="x" visual')).toBeNull();
		expect(visualFence("js", "visual")).toBeNull();
		expect(visualFence(undefined, "visual")).toBeNull();
	});

	it("writes a fence longer than any run of backticks in the page", () => {
		expect(toVisualFence({ html: "<p>hi</p>", title: "" })).toBe(
			"```html visual\n<p>hi</p>\n```"
		);
		const html = "<pre>````\ncode\n````</pre>";
		expect(toVisualFence({ html, title: "x" })).toBe(
			`\`\`\`\`\`html visual title="x"\n${html}\n\`\`\`\`\``
		);
	});

	it("escapes quotes and backslashes in the title and keeps it on one line", () => {
		const fence = toVisualFence({ html: "", title: 'Say "hi" \\ `now`\nplease' });
		expect(fence.split("\n")[0]).toBe(
			'```html visual title="Say \\"hi\\" \\\\ \'now\' please"'
		);
	});

	it("reads back what it writes", () => {
		for (const visual of [
			{ html: "<p>hi</p>", title: 'A "quoted" \\ title' },
			{ html: "a\n\n", title: "" },
			{ html: "", title: "Empty" },
			{ html: "```\n````\n", title: "Runs" },
		]) {
			expect(fromVisualFence(toVisualFence(visual))).toEqual(visual);
		}
	});

	it("reads a text that is exactly one visual fence", () => {
		expect(fromVisualFence('\n  ~~~html visual title="t"\n<b>x</b>\n~~~\n')).toEqual({
			html: "<b>x</b>",
			title: "t",
		});
		expect(fromVisualFence("```html visual\r\n<b>x</b>\r\n````")).toEqual({
			html: "<b>x</b>",
			title: "",
		});
	});

	it("refuses anything more or less than one visual fence", () => {
		expect(fromVisualFence("```html\n<b>x</b>\n```")).toBeNull();
		expect(fromVisualFence("```html visual\n<b>x</b>")).toBeNull();
		expect(fromVisualFence("```html visual\n<b>x</b>\n```\nmore")).toBeNull();
		expect(fromVisualFence("text\n```html visual\n<b>x</b>\n```")).toBeNull();
		expect(fromVisualFence("```html visual\na\n```\n```html visual\nb\n```")).toBeNull();
		expect(fromVisualFence("````html visual\na\n```")).toBeNull();
		expect(fromVisualFence("```html visual `x`\na\n```")).toBeNull();
	});
});

describe("the visuals in a text", () => {
	it("finds each finished visual and skips other code", () => {
		const text = [
			"Here:",
			'```html visual title="One"',
			"<p>1</p>",
			"```",
			"```js",
			"```html visual",
			"```",
			"~~~~html visual",
			"<p>2</p>",
			"~~~",
			"~~~~",
			"```html visual",
			"still coming",
		].join("\n");
		expect(visualsIn(text)).toEqual([
			{ html: "<p>1</p>", title: "One" },
			{ html: "<p>2</p>\n~~~", title: "" },
		]);
		expect(visualsIn("no fences here")).toEqual([]);
	});
});
