// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { idleReleaseOptions } from "./idle-release";

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
});
