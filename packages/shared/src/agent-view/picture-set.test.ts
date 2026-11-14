// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { pictureNeighbours, stepPicture } from "./picture-set";

describe("stepPicture", () => {
	it("moves one picture either way", () => {
		expect(stepPicture(1, 5, 1)).toBe(2);
		expect(stepPicture(1, 5, -1)).toBe(0);
	});

	it("stops at the ends instead of wrapping", () => {
		expect(stepPicture(0, 5, -1)).toBe(0);
		expect(stepPicture(4, 5, 1)).toBe(4);
	});
});

describe("pictureNeighbours", () => {
	it("lists the pictures either side within the set", () => {
		expect(pictureNeighbours(2, 5)).toEqual([1, 3]);
		expect(pictureNeighbours(0, 5)).toEqual([1]);
		expect(pictureNeighbours(4, 5)).toEqual([3]);
		expect(pictureNeighbours(0, 1)).toEqual([]);
	});
});
