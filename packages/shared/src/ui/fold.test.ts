// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { FOLD_SLACK, fitLevel, foldsAt } from "./fold";

// A row that needs 600 pixels as it is, 500 with one step folded, 420 with
// two, and 300 with all three.
const NEEDS = [600, 500, 420, 300];

describe("fitting a row on one line", () => {
	it("folds nothing where everything fits", () => {
		expect(fitLevel(800, NEEDS)).toBe(0);
		expect(fitLevel(600, NEEDS)).toBe(0);
	});

	it("folds as many steps as it takes to fit", () => {
		expect(fitLevel(599, NEEDS)).toBe(1);
		expect(fitLevel(450, NEEDS)).toBe(2);
		expect(fitLevel(310, NEEDS)).toBe(3);
	});

	it("stops at the last level even when that doesn't fit", () => {
		expect(fitLevel(200, NEEDS)).toBe(3);
	});

	it("unfolds only once the wider level fits with room to spare", () => {
		expect(fitLevel(599, NEEDS, 0)).toBe(1);
		expect(fitLevel(600, NEEDS, 1)).toBe(1);
		expect(fitLevel(600 + FOLD_SLACK - 1, NEEDS, 1)).toBe(1);
		expect(fitLevel(600 + FOLD_SLACK, NEEDS, 1)).toBe(0);
	});

	it("holds its level across a boundary, so a small resize doesn't flicker", () => {
		let level = fitLevel(495, NEEDS, 0);
		expect(level).toBe(2);
		for (const width of [503, 499, 505, 498, 507]) {
			level = fitLevel(width, NEEDS, level);
			expect(level).toBe(2);
		}
		expect(fitLevel(508, NEEDS, level)).toBe(1);
	});

	it("unfolds several known levels at once", () => {
		expect(fitLevel(1000, NEEDS, 3)).toBe(0);
	});

	it("tries a level it hasn't measured one step at a time", () => {
		const measured = [undefined, 500, undefined, undefined];
		expect(fitLevel(450, measured, 3)).toBe(2);
		expect(fitLevel(1000, measured, 2)).toBe(0);
		expect(fitLevel(1000, [700, undefined, undefined], 2)).toBe(1);
	});

	it("keeps a level it showed with room to spare", () => {
		const measured = [700, undefined, undefined];
		expect(fitLevel(650, measured, 1)).toBe(1);
		expect(fitLevel(650, measured, 0)).toBe(1);
	});

	it("names the steps folded at a level, in order", () => {
		expect([...foldsAt(["a", "b", "c"], 0)]).toEqual([]);
		expect([...foldsAt(["a", "b", "c"], 2)]).toEqual(["a", "b"]);
	});
});
