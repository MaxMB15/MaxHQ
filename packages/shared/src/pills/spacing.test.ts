// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Segment } from "./draft";
import { spaced } from "./spacing";

const file = (key: string): Segment => ({ type: "pill", pill: { type: "attachment", key } });
const text = (value: string): Segment => ({ type: "text", text: value });

describe("spacing an inserted pill", () => {
	it("adds a space before a pill that follows a word, and after the last pill", () => {
		expect(spaced([file("k1")], "look", "")).toEqual({
			segments: [text(" "), file("k1"), text(" ")],
			skipSpace: false,
		});
	});

	it("steps over a space that's already there", () => {
		expect(spaced([file("k1")], "look ", " here")).toEqual({
			segments: [file("k1")],
			skipSpace: true,
		});
	});

	it("leaves text alone unless asked", () => {
		expect(spaced([text("#412")], "see ", "").segments).toEqual([text("#412")]);
		expect(spaced([text("#412")], "see ", "", true).segments).toEqual([
			text("#412"),
			text(" "),
		]);
	});
});
