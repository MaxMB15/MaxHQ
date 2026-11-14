// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { fold, hasWord, hasWords, searchWords } from "./words";

describe("search words", () => {
	it("splits a search into words without case or accents", () => {
		expect(searchWords("  Rotate   URL ")).toEqual(["rotate", "url"]);
		expect(searchWords("Café")).toEqual(["cafe"]);
		expect(searchWords("   ")).toEqual([]);
	});

	it("keeps where each folded character came from", () => {
		expect(fold("Éa")).toEqual({ folded: "ea", from: [0, 1] });
		expect(fold("ﬁx")).toEqual({ folded: "fix", from: [0, 0, 1] });
	});

	it("finds a word ignoring case and accents", () => {
		expect(hasWord("Crème brûlée", "brulee")).toBe(true);
		expect(hasWord("Crème", "cream")).toBe(false);
	});

	it("needs every word, in any order", () => {
		expect(hasWords("Fix the Café menu", ["menu", "cafe"])).toBe(true);
		expect(hasWords("Fix the Café menu", ["menu", "bar"])).toBe(false);
		expect(hasWords("Anything", [])).toBe(true);
	});
});
