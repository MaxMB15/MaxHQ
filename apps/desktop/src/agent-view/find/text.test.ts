// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
	createFindPainter,
	FIND_ACTIVE_HIGHLIGHT,
	FIND_HIGHLIGHT,
	type FindPainter,
} from "./highlights";
import { collectRowRanges, readFindPosition } from "./text";

// A row as the list draws it, with its fields marked.
const rowOf = (html: string, item = "m1") => {
	const row = document.createElement("div");
	row.setAttribute("data-quote-item", item);
	row.innerHTML = html;
	document.body.append(row);
	return row;
};

const found = (row: Element, query: string) =>
	collectRowRanges(row, query).map((range) => range.toString());

afterEach(() => {
	document.body.innerHTML = "";
});

describe("collectRowRanges", () => {
	it("ignores case, as maxd folds it", () => {
		const row = rowOf(`<p data-find="plain">Deploy the DEPLOYMENT, İİ deploy and ΟΣ ος</p>`);
		expect(found(row, "deploy")).toEqual(["Deploy", "DEPLOY", "deploy"]);
		expect(found(row, "σ")).toEqual(["Σ", "ς"]);
	});

	it("runs on across inline elements and counts each run of whitespace as one space", () => {
		const row = rowOf(
			`<div data-find="markdown"><p>Run <strong>the  long</strong>\ttest <a href="#">suite</a></p></div>`
		);
		expect(found(row, "the long test suite")).toEqual(["the  long\ttest suite"]);
		// The query keeps the whitespace in it.
		expect(found(row, "the  long")).toEqual([]);
	});

	it("never crosses a block, a line break or a <br>", () => {
		const row = rowOf(
			`<div data-find="markdown"><p>one</p><p>two</p><ul><li>three</li><li>four</li></ul>` +
				`<table><tr><td>five</td><td>six</td></tr></table><p>seven<br>eight</p><p>nine\nten</p></div>`
		);
		for (const query of ["one two", "three four", "five six", "seven eight", "nine ten"])
			expect(found(row, query)).toEqual([]);
		expect(found(row, "four")).toEqual(["four"]);
	});

	it("reads a code block line by line, without its header", () => {
		const row = rowOf(
			`<div data-find="markdown"><div><div data-find-skip><span>ts</span><button>Copy</button></div>` +
				`<pre><code><span data-line>let a = 1;</span><span data-line>let b = 2;</span></code></pre></div></div>`
		);
		expect(found(row, "let")).toEqual(["let", "let"]);
		expect(found(row, "1; let")).toEqual([]);
		expect(found(row, "copy")).toEqual([]);
	});

	it("leaves pills out, and joins the text on either side", () => {
		const row = rowOf(
			`<p data-find="plain">look at <span data-piece="1" data-find-skip>notes.md</span> now</p>`
		);
		expect(found(row, "notes")).toEqual([]);
		// The match's range runs over the pill between its ends.
		expect(found(row, "at now")).toEqual(["at notes.md now"]);
	});

	it("counts only marked fields, field by field, and a field inside another once", () => {
		const row = rowOf(
			`<span>tool</span><pre data-find="plain">tool input</pre>` +
				`<div data-find="markdown"><p data-find="plain">tool output</p></div>` +
				`<button>tool</button><svg><text>tool</text></svg>`
		);
		expect(found(row, "tool")).toEqual(["tool", "tool"]);
		expect(collectRowRanges(row, "tool")[1]!.startContainer.textContent).toBe("tool output");
	});

	it("finds nothing for an empty query", () => {
		expect(found(rowOf(`<p data-find="plain">text</p>`), "")).toEqual([]);
	});
});

describe("readFindPosition", () => {
	it("starts at the first row in view", () => {
		const scroller = document.createElement("div");
		document.body.append(scroller);
		scroller.innerHTML =
			`<div data-quote-item="m1"><p data-find="plain">a a</p></div>` +
			`<div data-quote-item="m2"><p data-find="plain">a</p></div>`;
		const [above, shown] = scroller.querySelectorAll("[data-quote-item]");
		vi.spyOn(scroller, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 100, 800, 500));
		vi.spyOn(above!, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, 800, 80));
		vi.spyOn(shown!, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 90, 800, 80));
		Range.prototype.getBoundingClientRect = () => new DOMRect(0, 120, 10, 10);
		expect(readFindPosition(scroller, "a")).toEqual({ item: "m2", occurrence: 0 });
		// A row whose matches all sit above the view starts after them.
		Range.prototype.getBoundingClientRect = () => new DOMRect(0, 60, 10, 10);
		expect(readFindPosition(scroller, "a")).toEqual({ item: "m2", occurrence: 1 });
	});
});

class FakeHighlight extends Set<Range> {
	constructor(...ranges: Range[]) {
		super(ranges);
	}
}

const scope = globalThis as unknown as { CSS?: object; Highlight?: unknown };
const realCSS = scope.CSS;

const fakeHighlights = () => {
	Object.defineProperty(globalThis, "CSS", {
		value: { highlights: new Map() },
		configurable: true,
		writable: true,
	});
	scope.Highlight = FakeHighlight;
};

const lit = (name: string) =>
	[...((CSS.highlights.get(name) as unknown as Set<Range> | undefined) ?? [])].map((range) =>
		range.toString()
	);

describe("createFindPainter", () => {
	const painters: FindPainter[] = [];
	const createPainter = (scroller: HTMLElement) => {
		const painter = createFindPainter(scroller);
		painters.push(painter);
		return painter;
	};
	beforeEach(() => {
		vi.useFakeTimers({ toFake: ["requestAnimationFrame", "cancelAnimationFrame"] });
	});

	afterEach(() => {
		for (const painter of painters.splice(0)) painter.dispose();
		vi.useRealTimers();
		Object.defineProperty(globalThis, "CSS", {
			value: realCSS,
			configurable: true,
			writable: true,
		});
		delete scope.Highlight;
	});

	const scrollerOf = (html: string) => {
		const scroller = document.createElement("div");
		scroller.innerHTML = html;
		document.body.append(scroller);
		return scroller;
	};

	it("does nothing where the webview can't light ranges", () => {
		Object.defineProperty(globalThis, "CSS", { value: {}, configurable: true, writable: true });
		const painter = createPainter(
			scrollerOf(`<div data-quote-item="m1"><p data-find="plain">a</p></div>`)
		);
		expect(() => painter.paint("a", { item: "m1", occurrence: 0 })).not.toThrow();
		painter.dispose();
	});

	it("lights every match on screen and the active one apart", () => {
		fakeHighlights();
		const scroller = scrollerOf(
			`<div data-quote-item="m1"><p data-find="plain">Ab ab</p></div>` +
				`<div data-quote-item="m2"><p data-find="plain">aB</p></div>`
		);
		const painter = createPainter(scroller);
		painter.paint("ab", { item: "m1", occurrence: 1 });
		expect(lit(FIND_HIGHLIGHT)).toEqual(["Ab", "ab", "aB"]);
		expect(lit(FIND_ACTIVE_HIGHLIGHT)).toEqual(["ab"]);
		painter.paint("", null);
		expect(lit(FIND_HIGHLIGHT)).toEqual([]);
		expect(lit(FIND_ACTIVE_HIGHLIGHT)).toEqual([]);
		painter.dispose();
	});

	it("lights the row's last match when the row shows fewer than maxd counted", () => {
		fakeHighlights();
		const scroller = scrollerOf(
			`<div data-quote-item="m1"><p data-find="plain">a b a</p></div>`
		);
		const painter = createPainter(scroller);
		const row = scroller.firstElementChild!;
		painter.paint("a", { item: "m1", occurrence: 7 });
		expect(lit(FIND_ACTIVE_HIGHLIGHT)).toEqual(["a"]);
		expect(painter.rangeIn(row, { item: "m1", occurrence: 7 })?.startOffset).toBe(4);
		expect(painter.rangeIn(row, { item: "m1", occurrence: 0 })?.startOffset).toBe(0);
		painter.dispose();
	});

	it("paints again on the next frame when a row comes or its text changes", async () => {
		fakeHighlights();
		const scroller = scrollerOf(`<div data-quote-item="m1"><p data-find="plain">one</p></div>`);
		const painter = createPainter(scroller);
		painter.paint("one", null);
		scroller.insertAdjacentHTML(
			"beforeend",
			`<div data-quote-item="m2"><p data-find="plain">one more</p></div>`
		);
		scroller.querySelector("[data-quote-item=m1] p")!.firstChild!.textContent = "one, one";
		// The observer hears the changes once the task ends.
		await Promise.resolve();
		vi.advanceTimersToNextFrame();
		expect(lit(FIND_HIGHLIGHT)).toEqual(["one", "one", "one"]);
		painter.dispose();
		expect(lit(FIND_HIGHLIGHT)).toEqual([]);
	});

	it("shares the window's highlights between views", () => {
		fakeHighlights();
		const first = createPainter(
			scrollerOf(`<div data-quote-item="m1"><p data-find="plain">x</p></div>`)
		);
		const second = createPainter(
			scrollerOf(`<div data-quote-item="m9"><p data-find="plain">x y</p></div>`)
		);
		first.paint("x", null);
		second.paint("y", null);
		expect(lit(FIND_HIGHLIGHT)).toEqual(["x", "y"]);
		first.dispose();
		expect(lit(FIND_HIGHLIGHT)).toEqual(["y"]);
		second.dispose();
	});
});
