// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import {
	shownValue,
	type Stop,
	stopAt,
	stopCenter,
	stopIndex,
	stopText,
	stopToPick,
} from "./step-slider";

const stops: Stop[] = [
	{ value: "low", label: "Low" },
	{ value: "medium", label: "Medium", isDefault: true },
	{ value: "high", label: "High" },
];

describe("step slider", () => {
	it("shows a pick until the value moves away from what it was then", () => {
		expect(shownValue(null, "low")).toBe("low");
		expect(shownValue({ from: "low", to: "high" }, "low")).toBe("high");
		expect(shownValue({ from: "low", to: "high" }, "high")).toBe("high");
		expect(shownValue({ from: "low", to: "high" }, "medium")).toBe("medium");
	});

	it("finds a value's stop, or the first for one it doesn't have", () => {
		expect(stopIndex(stops, "high")).toBe(2);
		expect(stopIndex(stops, "")).toBe(0);
	});

	it("picks a stop held to the ends, and nothing for the one shown", () => {
		expect(stopToPick(stops, 2, "low")?.value).toBe("high");
		expect(stopToPick(stops, 5, "low")?.value).toBe("high");
		expect(stopToPick(stops, -1, "high")?.value).toBe("low");
		expect(stopToPick(stops, 3, "high")).toBeNull();
		expect(stopToPick([], 0, "")).toBeNull();
	});

	it("finds the stop under a point, each stop a column of the same width", () => {
		expect(stopAt(0, 300, 3)).toBe(0);
		expect(stopAt(99, 300, 3)).toBe(0);
		expect(stopAt(100, 300, 3)).toBe(1);
		expect(stopAt(299, 300, 3)).toBe(2);
		expect(stopAt(300, 300, 3)).toBe(2);
		expect(stopAt(-40, 300, 3)).toBe(0);
		expect(stopAt(900, 300, 3)).toBe(2);
	});

	it("puts each stop at the center of its column", () => {
		expect(stopCenter(0, 4)).toBe(0.125);
		expect(stopCenter(3, 4)).toBe(0.875);
	});

	it("names a stop, and marks the default", () => {
		expect(stops.map(stopText)).toEqual(["Low", "Medium (default)", "High"]);
	});
});
