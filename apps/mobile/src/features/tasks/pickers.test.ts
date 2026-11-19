// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { pickedLabel, pickerRows } from "./pickers";

const options = [
	{ value: "a", label: "Atlas", group: "Projects" },
	{ value: "b", label: "Beacon", group: "Projects" },
	{ value: "*folder", label: "Use a local folder" },
];

describe("a picker's list", () => {
	it("puts a heading over each group, once", () => {
		expect(pickerRows(options).map((row) => row.key)).toEqual([
			"g:Projects",
			"o:a",
			"o:b",
			"o:*folder",
		]);
	});

	it("shows the picked option's label, or the empty word", () => {
		expect(pickedLabel(options, "b", "None")).toBe("Beacon");
		expect(pickedLabel(options, "gone", "None")).toBe("None");
	});
});
