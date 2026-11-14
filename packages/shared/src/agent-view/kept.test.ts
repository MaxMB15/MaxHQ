// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { beforeEach, describe, expect, it } from "vitest";

import { clearKept, keep, keptValue } from "./kept";

describe("keep", () => {
	beforeEach(clearKept);

	it("keeps each value under its own key", () => {
		keep("draft:a1", "Fix the meter");
		expect(keptValue("draft:a1", "")).toBe("Fix the meter");
		expect(keptValue("draft:a2", "")).toBe("");
	});

	it("drops the oldest value past the limit", () => {
		for (let i = 0; i <= 2000; i++) keep(`open:${i}`, true);
		expect(keptValue("open:0", false)).toBe(false);
		expect(keptValue("open:2000", false)).toBe(true);
	});
});
