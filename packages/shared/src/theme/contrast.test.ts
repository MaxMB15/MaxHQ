// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { contrast, over, parseColour, type Rgba } from "./contrast";

const parsed = (text: string): Rgba => {
	const colour = parseColour(text);
	if (!colour) throw new Error(`couldn't read ${text}`);
	return colour;
};

const bytes = ({ r, g, b, a }: Rgba) =>
	[r, g, b].map((x) => Math.round(x * 255)).concat(Math.round(a * 1000) / 1000);

describe("reading a colour", () => {
	it("reads hex in each length", () => {
		expect(bytes(parsed("#4692f6"))).toEqual([70, 146, 246, 1]);
		expect(bytes(parsed("#fff"))).toEqual([255, 255, 255, 1]);
		expect(parsed("#00000080").a).toBeCloseTo(0.5, 2);
	});

	it("reads rgb in both syntaxes", () => {
		expect(bytes(parsed("rgb(70 146 246 / 0.14)"))).toEqual([70, 146, 246, 0.14]);
		expect(bytes(parsed("rgba(0,0,0,0.75)"))).toEqual([0, 0, 0, 0.75]);
	});

	it("reads oklch as sRGB", () => {
		expect(bytes(parsed("oklch(1 0 0)"))).toEqual([255, 255, 255, 1]);
		expect(bytes(parsed("oklch(0 0 0)"))).toEqual([0, 0, 0, 1]);
		// Pure red, as OKLCH writes it.
		expect(bytes(parsed("oklch(62.8% 0.2577 29.23)"))).toEqual([255, 0, 0, 1]);
		expect(parsed("oklch(0.62 0.136 237 / 0.35)").a).toBe(0.35);
	});

	it("mixes in sRGB, with a share on either side", () => {
		expect(bytes(parsed("color-mix(in srgb, #ffffff 50%, #000000)"))).toEqual([
			128, 128, 128, 1,
		]);
		expect(parsed("color-mix(in srgb, #3fb37f 55%, transparent)").a).toBeCloseTo(0.55);
		expect(bytes(parsed("color-mix(in srgb, transparent, #3fb37f 20%)"))).toEqual([
			63, 179, 127, 0.2,
		]);
	});

	it("gives null for a form it doesn't read", () => {
		expect(parseColour("var(--primary)")).toBeNull();
		expect(parseColour("hsl(0 0% 0%)")).toBeNull();
	});
});

describe("contrast", () => {
	it("runs from 1 to 21", () => {
		expect(contrast(parsed("white"), parsed("black"))).toBeCloseTo(21);
		expect(contrast(parsed("#777"), parsed("#777"))).toBe(1);
	});

	it("matches WCAG's figure for a known pair", () => {
		expect(contrast(parsed("#767676"), parsed("white"))).toBeCloseTo(4.54, 2);
	});

	it("measures a see-through colour over what's under it", () => {
		const tint = over(parsed("rgb(0 0 0 / 0.5)"), parsed("white"));
		expect(bytes(tint)).toEqual([128, 128, 128, 1]);
	});
});
