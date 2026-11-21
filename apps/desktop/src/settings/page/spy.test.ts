// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { currentGroup, SPY_LINE } from "./spy";

const box = { top: 0, height: 600, scrollHeight: 3000 };

describe("the group you're in", () => {
	it("is the first at the top", () => {
		expect(currentGroup([0, 400, 900], box)).toBe(0);
	});

	it("is the last group whose heading reached the line", () => {
		expect(currentGroup([-500, -100, SPY_LINE, 700], { ...box, top: 500 })).toBe(2);
		expect(currentGroup([-500, -100, SPY_LINE + 1, 700], { ...box, top: 500 })).toBe(1);
	});

	it("is the last group once the page can't scroll further", () => {
		expect(currentGroup([-2000, -900, 300], { ...box, top: 2400 })).toBe(2);
	});

	it("is none with no groups", () => {
		expect(currentGroup([], box)).toBe(-1);
	});

	it("stays on the first when the page doesn't scroll", () => {
		expect(currentGroup([0, 200], { top: 0, height: 600, scrollHeight: 600 })).toBe(0);
	});
});
