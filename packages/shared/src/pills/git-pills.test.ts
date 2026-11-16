// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { GitItemRef, GitTarget } from "./draft";
import { createGitPills, GIT_LOOKUP_DELAY_MS, shownItem, shownItems } from "./git-pills";

const item = (number: number): GitItemRef =>
	({
		type: "git_item",
		repo: "owner/repo",
		number,
		kind: "issue",
		state: "open",
		title: `Item ${number}`,
		url: `https://example.com/issues/${number}`,
	}) as GitItemRef;

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("createGitPills", () => {
	it("looks each new number up after a pause and shows the ones it finds", async () => {
		const asked: GitTarget[] = [];
		const store = createGitPills(async (target) => {
			asked.push(target);
			return target.number === 4 ? item(4) : null;
		});
		store.getState().want([
			{ repo: null, number: 4 },
			{ repo: null, number: 5 },
		]);
		expect(asked).toEqual([]);
		await vi.advanceTimersByTimeAsync(GIT_LOOKUP_DELAY_MS);
		expect(asked).toHaveLength(2);
		expect(shownItem(store.getState(), "#4")).toEqual(item(4));
		expect(store.getState().lookups.get("#5")).toEqual({ state: "missing" });
	});

	it("drops a lookup for a number typed away before the pause ends", async () => {
		const asked: GitTarget[] = [];
		const store = createGitPills(async (target) => {
			asked.push(target);
			return null;
		});
		store.getState().want([{ repo: null, number: 1 }]);
		store.getState().want([{ repo: null, number: 12 }]);
		await vi.advanceTimersByTimeAsync(GIT_LOOKUP_DELAY_MS);
		expect(asked).toEqual([{ repo: null, number: 12 }]);
	});

	it("shows a seeded item at once, and stops showing one kept as text", () => {
		const store = createGitPills(async () => null);
		store.getState().seed(new Map([["#9", item(9)]]));
		expect(shownItems(store.getState(), "see #9 and #9")).toEqual([item(9)]);
		store.getState().keep("#9");
		expect(shownItems(store.getState(), "see #9")).toEqual([]);
		store.getState().seed(new Map([["#9", item(9)]]));
		expect(shownItem(store.getState(), "#9")).toEqual(item(9));
	});
});
