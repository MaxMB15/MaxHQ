// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { idleReleaseOptions, idleReleaseText } from "./idle-release";

describe("idle release", () => {
	it("offers never and 15 minutes to 8 hours", () => {
		expect(idleReleaseOptions(60).map((o) => o.label)).toEqual([
			"Never",
			"15 minutes",
			"30 minutes",
			"1 hour",
			"2 hours",
			"4 hours",
			"8 hours",
		]);
		expect(idleReleaseOptions(60).map((o) => o.value)).toEqual([
			"0",
			"15",
			"30",
			"60",
			"120",
			"240",
			"480",
		]);
	});

	it("keeps a time set elsewhere among the picks", () => {
		const options = idleReleaseOptions(45);
		expect(options.map((o) => o.label)).toContain("45 minutes");
		expect(options.findIndex((o) => o.value === "45")).toBe(3);
	});

	it("words minutes as minutes or hours", () => {
		expect(idleReleaseText(0)).toBe("Never");
		expect(idleReleaseText(1)).toBe("1 minute");
		expect(idleReleaseText(90)).toBe("90 minutes");
		expect(idleReleaseText(60)).toBe("1 hour");
		expect(idleReleaseText(480)).toBe("8 hours");
	});
});
