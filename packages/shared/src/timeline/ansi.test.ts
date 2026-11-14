// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { ansiBlocks, applySgr, paletteColor, parseAnsi, PLAIN, stripAnsi } from "./ansi";

const E = String.fromCharCode(27);

describe("ansiBlocks", () => {
	const text = (blocks: ReturnType<typeof ansiBlocks>) =>
		blocks.map((spans) => spans.map((span) => span.text).join(""));

	it("keeps output under the limit in one block", () => {
		expect(text(ansiBlocks(parseAnsi("a\nb\nc"), 5))).toEqual(["a\nb\nc"]);
	});

	it("breaks between lines at the limit, losing no line", () => {
		expect(text(ansiBlocks(parseAnsi("a\nb\nc\nd\ne"), 2))).toEqual(["a\nb", "c\nd", "e"]);
	});

	it("keeps a run's style on both sides of a break", () => {
		const blocks = ansiBlocks(parseAnsi(`${E}[31ma\nb\nc${E}[0m`), 1);
		expect(text(blocks)).toEqual(["a", "b", "c"]);
		const red = parseAnsi(`${E}[31mx`)[0]!.style;
		for (const [span] of blocks) expect(span?.style).toEqual(red);
	});

	it("counts lines across runs", () => {
		const blocks = ansiBlocks(parseAnsi(`a\n${E}[31mb\nc`), 2);
		expect(text(blocks)).toEqual(["a\nb", "c"]);
	});

	it("gives no block for nothing", () => {
		expect(ansiBlocks([], 2)).toEqual([]);
	});
});

describe("parseAnsi", () => {
	it("leaves plain text in one run", () => {
		expect(parseAnsi("ok\ndone")).toEqual([{ text: "ok\ndone", style: PLAIN }]);
	});

	it("styles the text after a colour and resets", () => {
		const spans = parseAnsi(`a ${E}[31mred${E}[0m b`);
		expect(spans.map((s) => s.text)).toEqual(["a ", "red", " b"]);
		expect(spans[1]!.style.fg).toBe(paletteColor(1));
		expect(spans[2]!.style).toEqual(PLAIN);
	});

	it("reads bold, bright, 256 and true colours", () => {
		const [bold] = parseAnsi(`${E}[1;92mx`);
		expect(bold!.style).toMatchObject({ bold: true, fg: paletteColor(10) });
		expect(parseAnsi(`${E}[38;5;196mx`)[0]!.style.fg).toBe("#ff0000");
		expect(parseAnsi(`${E}[48;2;1;2;3mx`)[0]!.style.bg).toBe("#010203");
		expect(parseAnsi(`${E}[38:5:232mx`)[0]!.style.fg).toBe("#080808");
	});

	it("drops escapes that aren't colours", () => {
		expect(stripAnsi(`${E}[2K${E}[1Gline${E}]0;title${String.fromCharCode(7)}!`)).toBe("line!");
		expect(stripAnsi(`end${E}`)).toBe("end");
	});

	it("keeps what a line shows after its last carriage return", () => {
		expect(stripAnsi("10%\r50%\r100%\nnext\r\n")).toBe("100%\nnext\n");
	});

	it("merges runs that look the same", () => {
		expect(parseAnsi(`${E}[31ma${E}[31mb`)).toHaveLength(1);
	});
});

describe("applySgr", () => {
	it("turns styles off one at a time", () => {
		const on = applySgr(PLAIN, [1, 3, 4, 33, 44]);
		expect(applySgr(on, [22, 23, 24, 39, 49])).toEqual(PLAIN);
	});

	it("skips a malformed extended colour", () => {
		expect(applySgr(PLAIN, [38, 5])).toEqual(PLAIN);
		expect(applySgr(PLAIN, [38, 2, 300, 0, 0])).toEqual(PLAIN);
	});
});
