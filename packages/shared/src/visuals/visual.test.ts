// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { fnv1a } from "../lib/fnv";
import { MAX_VISUAL_BYTES, visualBytes, visualFileName, visualKey } from "./visual";

describe("visuals", () => {
	it("keys a page by its length and FNV-1a, the same on every run", () => {
		expect(fnv1a("")).toBe(0x811c9dc5);
		expect(fnv1a("a")).toBe(0xe40c292c);
		expect(visualKey("a")).toBe(`1-${(0xe40c292c).toString(36)}`);
		expect(visualKey("<p>one</p>")).not.toBe(visualKey("<p>two</p>"));
	});

	it("counts a page's UTF-8 bytes", () => {
		for (const text of ["", "abc", "é", "€", "😀", "a😀é€", "\ud800x", "x\udc00"]) {
			expect(visualBytes(text), text).toBe(new TextEncoder().encode(text).length);
		}
		expect(MAX_VISUAL_BYTES).toBe(524_288);
	});

	it("names the saved file after the title", () => {
		expect(visualFileName("Token use by day")).toBe("visual-token-use-by-day.html");
		expect(visualFileName("  Café: Q3 / Q4!  ")).toBe("visual-cafe-q3-q4.html");
		expect(visualFileName("")).toBe("visual.html");
		expect(visualFileName("日本")).toBe("visual.html");
		expect(visualFileName("x".repeat(100))).toBe(`visual-${"x".repeat(60)}.html`);
	});
});
