// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { minuteOptions, minutesText } from "./minutes";

describe("minutes", () => {
	it("words minutes as minutes or hours", () => {
		expect(minutesText(0)).toBe("Never");
		expect(minutesText(1)).toBe("1 minute");
		expect(minutesText(90)).toBe("90 minutes");
		expect(minutesText(60)).toBe("1 hour");
		expect(minutesText(480)).toBe("8 hours");
	});

	it("keeps Never where the offered times put it", () => {
		expect(minuteOptions([0, 15, 60], 15).map((o) => o.value)).toEqual(["0", "15", "60"]);
		expect(minuteOptions([15, 60, 0], 15).map((o) => o.value)).toEqual(["15", "60", "0"]);
	});

	it("adds a time set elsewhere in its place", () => {
		expect(minuteOptions([15, 60, 0], 45).map((o) => o.label)).toEqual([
			"15 minutes",
			"45 minutes",
			"1 hour",
			"Never",
		]);
	});
});
