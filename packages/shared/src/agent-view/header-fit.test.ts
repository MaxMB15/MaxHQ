// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { fitLevel } from "../ui/fold";
import { foldedAt, headerNeeds } from "./header-fit";

const foldLevel = (width: number, chips = 0, level = 0) =>
	fitLevel(width, headerNeeds(chips), level);

const folded = (width: number) => [...foldedAt(foldLevel(width))];

describe("header folds", () => {
	it("shows every label in a wide pane", () => {
		expect(folded(1400)).toEqual([]);
	});

	it("folds Open in first, then the context, the branch, the device and the folder", () => {
		expect(folded(900)).toEqual(["openIn"]);
		expect(folded(800)).toEqual(["openIn", "context"]);
		expect(folded(700)).toEqual(["openIn", "context", "branch"]);
		expect(folded(600)).toEqual(["openIn", "context", "branch", "device"]);
		expect(folded(560)).toEqual(["openIn", "context", "branch", "device", "folder"]);
	});

	it("folds the account, then drops the model last", () => {
		expect(folded(500)).toContain("account");
		expect(folded(500)).not.toContain("model");
		expect(folded(320)).toHaveLength(7);
		expect(folded(320).at(-1)).toBe("model");
	});

	it("folds sooner for each chip after the model", () => {
		expect(foldLevel(900, 0)).toBe(1);
		expect(foldLevel(900, 1)).toBe(2);
		expect(foldLevel(900, 2)).toBe(3);
	});

	it("keeps the same level across a range, so a resize renders only at a step", () => {
		expect(foldLevel(700)).toBe(foldLevel(759));
		expect(foldLevel(760)).toBe(foldLevel(700) - 1);
	});

	it("unfolds a label only once it fits with room to spare", () => {
		const level = foldLevel(750);
		expect(foldLevel(762, 0, level)).toBe(level);
		expect(foldLevel(770, 0, level)).toBe(level - 1);
		expect(headerNeeds(1)[0]).toBe(960 + 120);
	});
});
