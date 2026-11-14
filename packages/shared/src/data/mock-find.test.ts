// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { findMockGitItems, findMockPaths } from "./mock-find";

const NOW = 1_793_361_600_000;

describe("the sample menus", () => {
	it("finds paths by their characters in order", () => {
		expect(findMockPaths("cach").map((match) => match.path)).toEqual([
			"src/cache.rs",
			"tests/cache.rs",
		]);
		expect(findMockPaths("").length).toBeGreaterThan(5);
	});

	it("finds pull requests and issues by number or title", () => {
		const byNumber = findMockGitItems("owner/repo", "41", NOW);
		expect(byNumber.items.map((item) => item.number)).toEqual([412]);
		expect(byNumber.items[0]?.url).toBe("https://github.com/owner/repo/pull/412");
		expect(
			findMockGitItems("owner/repo", "usage meter", NOW).items.map((item) => item.number)
		).toEqual([405]);
	});

	it("says why an agent with no repository finds nothing", () => {
		expect(findMockGitItems(null, "4", NOW)).toEqual({
			repo: null,
			items: [],
			unavailable: "This agent's folder has no GitHub repository",
		});
	});
});
