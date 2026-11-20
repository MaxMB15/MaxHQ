// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { SCREEN_TALLEST, SCREEN_WAITING, screenHeight } from "./screen";

describe("screenHeight", () => {
	it("keeps the picture's shape across the box", () => {
		expect(screenHeight(360, 16 / 9)).toBe(203);
	});

	it("stops a tall picture at the tallest", () => {
		expect(screenHeight(360, 0.46)).toBe(SCREEN_TALLEST);
	});

	it("waits at a fixed height until the box and the picture are known", () => {
		expect(screenHeight(0, 1)).toBe(SCREEN_WAITING);
		expect(screenHeight(360, null)).toBe(SCREEN_WAITING);
	});
});
