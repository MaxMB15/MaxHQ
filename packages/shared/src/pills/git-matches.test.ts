// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { GitItemRef } from "./draft";
import { matchesGitQuery, rankGitItems } from "./git-matches";

const item = (number: number, title: string): GitItemRef => ({
	type: "git_item",
	repo: "owner/repo",
	number,
	kind: "pull_request",
	title,
	state: "open",
	url: `https://github.com/owner/repo/pull/${number}`,
	head: null,
	base: null,
	body: "",
	checked_at: 1_793_361_600_000,
});

const numbers = (items: GitItemRef[]) => items.map((found) => found.number);

describe("matchesGitQuery", () => {
	it("matches digits in the number and words in the title", () => {
		expect(matchesGitQuery(item(412, "Cache usage"), "41")).toBe(true);
		expect(matchesGitQuery(item(412, "Cache usage"), "9")).toBe(false);
		expect(matchesGitQuery(item(412, "Cache usage between polls"), "cache-polls")).toBe(true);
		expect(matchesGitQuery(item(412, "Cache usage"), "sidebar")).toBe(false);
	});
});

describe("rankGitItems", () => {
	it("puts the exact number first, then linked items, once each", () => {
		const items = [item(4120, "Later"), item(41, "Other"), item(412, "Cache")];
		const linked = [item(41, "Other")];
		expect(numbers(rankGitItems({ items, linked, query: "412", limit: 5 }))).toEqual([
			412, 4120,
		]);
		expect(numbers(rankGitItems({ items, linked, query: "41", limit: 5 }))).toEqual([
			41, 4120, 412,
		]);
		expect(numbers(rankGitItems({ items, query: "", limit: 2 }))).toEqual([4120, 41]);
	});
});
