// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { DEVBOX } from "../data/mock-view";

import { covers, type Found } from "./field";

/** A listing of ~/code asked for `filter`, whole or cut short. */
const found = (filter: string, truncated: boolean): Found => ({
	device: DEVBOX,
	folder: "~/code",
	files: false,
	filter,
	round: 0,
	listing: { path: "/home/dev/code", home: "/home/dev", exists: true, entries: [], truncated },
});

describe("covers", () => {
	it("answers for the name it was asked for, in any case", () => {
		expect(covers(found("du", true), "Du")).toBe(true);
	});

	it("answers for more of the name when it came whole", () => {
		expect(covers(found("d", false), "Dusk")).toBe(true);
		expect(covers(found("", false), "notes")).toBe(true);
	});

	it("asks again when it was cut short, so a name past the cut still comes", () => {
		expect(covers(found("", true), "zebra")).toBe(false);
		expect(covers(found("d", true), "du")).toBe(false);
	});

	it("asks again for less of the name, and when no listing came", () => {
		expect(covers(found("du", false), "d")).toBe(false);
		expect(covers({ ...found("d", false), listing: null }, "du")).toBe(false);
	});
});
