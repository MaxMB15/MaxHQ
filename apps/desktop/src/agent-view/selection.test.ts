// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it } from "vitest";

import { captureRowSelection, readRowText, resolveQuoteRange } from "./selection";

// jsdom doesn't pass `user-select` down, so each unselectable piece of text
// says so itself, the way a browser's computed style would.
const timeline = (html: string): HTMLElement => {
	document.body.innerHTML = `<main>${html}</main>`;
	return document.querySelector("main")!;
};

const textOf = (selector: string): Text => {
	const element = document.querySelector(selector)!;
	return element.firstChild as Text;
};

const select = (start: [Node, number], end: [Node, number]): Selection => {
	const selection = document.getSelection()!;
	selection.setBaseAndExtent(start[0], start[1], end[0], end[1]);
	return selection;
};

afterEach(() => {
	document.getSelection()?.removeAllRanges();
	document.body.innerHTML = "";
});

describe("readRowText", () => {
	it("puts a line break between blocks and skips text that can't be picked", () => {
		const row = timeline(`
			<div data-quote-item="i1"><span style="user-select: none">Ran</span><span>cargo test</span><div><pre>ok\nall passed</pre></div><button style="user-select: none">Copy</button></div>
		`).querySelector<HTMLElement>("[data-quote-item]")!;
		expect(readRowText(row).text).toBe("cargo test\nok\nall passed");
	});

	it("leaves out form fields and hidden text", () => {
		const row = timeline(
			`<div data-quote-item="i1"><p>Pick one</p><input value="typed"><span hidden>secret</span><span aria-hidden="true">icon</span><p>Next</p></div>`
		).querySelector<HTMLElement>("[data-quote-item]")!;
		expect(readRowText(row).text).toBe("Pick one\nNext");
	});

	it("reads highlighted code's lines apart, as the plain code's line breaks do", () => {
		const row = timeline(
			`<div data-quote-item="i1"><pre><code><span data-line class="block"><span>let a = 1;</span></span><span data-line class="block"></span><span data-line class="block"><span>let b</span><span> = 2;</span></span></code></pre></div>`
		).querySelector<HTMLElement>("[data-quote-item]")!;
		expect(readRowText(row).text).toBe("let a = 1;\nlet b = 2;");
	});
});

describe("captureRowSelection", () => {
	it("maps a selection in a row to its item and passage", () => {
		const viewport = timeline(
			`<div data-quote-item="i1"><p id="a">The first paragraph.</p><p id="b">The second one, with the part you want.</p></div>`
		);
		const text = textOf("#b");
		const from = text.data.indexOf("the part");
		const captured = captureRowSelection(viewport, select([text, from], [text, from + 8]));
		expect(captured?.item).toBe("i1");
		expect(captured?.anchor).toEqual({
			text: "the part",
			position: "The first paragraph. The second one, with ".length,
		});
		expect(captured?.range.toString()).toBe("the part");
	});

	it("takes a selection that ends at the start of the next row", () => {
		const viewport = timeline(
			`<div data-quote-item="i1"><p id="a">All of this.</p></div><div data-quote-item="i2"><p id="b">Not this.</p></div>`
		);
		const captured = captureRowSelection(
			viewport,
			select([textOf("#a"), 0], [textOf("#b"), 0])
		);
		expect(captured?.item).toBe("i1");
		expect(captured?.anchor.text).toBe("All of this.");
	});

	it("passes over the unselectable text a triple click picks up", () => {
		const viewport = timeline(
			`<div data-quote-item="i1"><p id="a">Recording your answers.</p></div><div data-quote-item="i2"><span id="verb" style="user-select: none">Ran</span></div><div data-quote-item="i3"><p id="c">Next reply.</p></div>`
		);
		const captured = captureRowSelection(
			viewport,
			select([textOf("#a"), 0], [textOf("#c"), 0])
		);
		expect(captured?.item).toBe("i1");
		expect(captured?.anchor.text).toBe("Recording your answers.");
	});

	it("gives nothing for a selection across two rows", () => {
		const viewport = timeline(
			`<div data-quote-item="i1"><p id="a">First row.</p></div><div data-quote-item="i2"><p id="b">Second row.</p></div>`
		);
		expect(
			captureRowSelection(viewport, select([textOf("#a"), 0], [textOf("#b"), 3]))
		).toBeNull();
	});

	it("gives nothing outside a row, outside the timeline, or when nothing is picked", () => {
		const viewport = timeline(
			`<p id="loose">Not in a row.</p><div data-quote-item="i1"><p id="a">Row.</p></div>`
		);
		expect(
			captureRowSelection(viewport, select([textOf("#loose"), 0], [textOf("#loose"), 3]))
		).toBeNull();
		expect(
			captureRowSelection(viewport, select([textOf("#a"), 1], [textOf("#a"), 1]))
		).toBeNull();
		expect(captureRowSelection(viewport, null)).toBeNull();
		const elsewhere = document.createElement("div");
		expect(
			captureRowSelection(elsewhere, select([textOf("#a"), 0], [textOf("#a"), 3]))
		).toBeNull();
	});

	it("works for a selection made backwards", () => {
		const viewport = timeline(
			`<div data-quote-item="i1"><p id="a">Pick these words here.</p></div>`
		);
		const text = textOf("#a");
		const captured = captureRowSelection(viewport, select([text, 16], [text, 5]));
		expect(captured?.anchor.text).toBe("these words");
	});
});

describe("resolveQuoteRange", () => {
	it("finds a quoted passage again across blocks", () => {
		const viewport = timeline(
			`<div data-quote-item="i1"><p id="a">Intro line.</p><p id="b">Start of the quote</p><p id="c">and its end.</p></div>`
		);
		const row = viewport.querySelector<HTMLElement>("[data-quote-item]")!;
		const made = captureRowSelection(viewport, select([textOf("#b"), 9], [textOf("#c"), 7]))!;
		document.getSelection()?.removeAllRanges();
		const range = resolveQuoteRange(row, made.anchor);
		expect(range?.startContainer).toBe(textOf("#b"));
		expect(range?.startOffset).toBe(9);
		expect(range?.endContainer).toBe(textOf("#c"));
		expect(range?.endOffset).toBe(7);
	});

	it("gives nothing when the row no longer has the passage", () => {
		const row = timeline(
			`<div data-quote-item="i1"><p>Different text now.</p></div>`
		).querySelector<HTMLElement>("[data-quote-item]")!;
		expect(resolveQuoteRange(row, { text: "the quote", position: 0 })).toBeNull();
	});
});
