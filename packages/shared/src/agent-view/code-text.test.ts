// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import {
	BINARY_SNIFF,
	colourable,
	HIGHLIGHT_BYTES,
	HIGHLIGHT_LINES,
	looksBinary,
	textLines,
} from "./code-text";

describe("looksBinary", () => {
	it("calls a file with a zero byte early on binary, and one later on text", () => {
		expect(looksBinary(Uint8Array.from([111, 107, 0, 109]))).toBe(true);
		const late = new Uint8Array(BINARY_SNIFF + 1).fill(120);
		late[BINARY_SNIFF] = 0;
		expect(looksBinary(late)).toBe(false);
		expect(looksBinary(new Uint8Array())).toBe(false);
	});
});

describe("textLines", () => {
	it("splits lines of any ending, without an empty one for the last break", () => {
		expect(textLines("a\r\nb\rc\n")).toEqual(["a", "b", "c"]);
		expect(textLines("")).toEqual([""]);
		expect(textLines("\n")).toEqual([""]);
		expect(textLines("a\n\n")).toEqual(["a", ""]);
	});
});

describe("colourable", () => {
	it("leaves a long file without colours", () => {
		expect(colourable({ lines: ["a"], bytes: HIGHLIGHT_BYTES })).toBe(true);
		expect(colourable({ lines: ["a"], bytes: HIGHLIGHT_BYTES + 1 })).toBe(false);
		const many = Array.from({ length: HIGHLIGHT_LINES + 1 }, () => "");
		expect(colourable({ lines: many, bytes: many.length })).toBe(false);
		expect(colourable({ lines: many.slice(1), bytes: many.length })).toBe(true);
	});
});
