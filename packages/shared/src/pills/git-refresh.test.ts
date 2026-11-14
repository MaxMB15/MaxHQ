// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it, vi } from "vitest";

import type { GitItemRef } from "./draft";
import { GIT_REFRESH_TIMEOUT_MS, GIT_STALE_MS, refreshGitItems } from "./git-refresh";

const NOW = 1_793_361_600_000;
const MIN = 60_000;

const pr = (number: number, checked_at: number, state: GitItemRef["state"] = "open") =>
	({
		type: "git_item",
		repo: "owner/repo",
		number,
		kind: "pull_request",
		title: "Cache usage between polls",
		state,
		url: `https://github.com/owner/repo/pull/${number}`,
		head: "usage-cache",
		base: "main",
		body: "",
		checked_at,
	}) satisfies GitItemRef;

afterEach(() => {
	vi.useRealTimers();
});

describe("refreshGitItems", () => {
	it("leaves an item checked in the last five minutes and doesn't look it up", async () => {
		const find = vi.fn(async () => pr(412, NOW, "merged"));
		const items = [pr(412, NOW - GIT_STALE_MS)];
		expect(await refreshGitItems(items, find, NOW)).toEqual(items);
		expect(find).not.toHaveBeenCalled();
	});

	it("swaps an older item for what its lookup finds now", async () => {
		const find = vi.fn(async (item: GitItemRef) => pr(item.number, NOW, "merged"));
		const fresh = pr(409, NOW - MIN);
		const stale = pr(412, NOW - 6 * MIN);
		expect(await refreshGitItems([stale, fresh], find, NOW)).toEqual([
			pr(412, NOW, "merged"),
			fresh,
		]);
		expect(find).toHaveBeenCalledTimes(1);
		expect(find).toHaveBeenCalledWith(stale);
	});

	it("keeps an older item when its lookup fails or finds nothing", async () => {
		const items = [pr(412, NOW - 10 * MIN), pr(398, NOW - 10 * MIN)];
		const find = vi.fn(async ({ number }: GitItemRef) => {
			if (number === 412) throw new Error("No network");
			return null;
		});
		expect(await refreshGitItems(items, find, NOW)).toEqual(items);
	});

	it("looks the items up side by side and keeps one whose lookup is slow", async () => {
		vi.useFakeTimers();
		const items = [pr(412, NOW - 10 * MIN), pr(398, NOW - 10 * MIN)];
		const find = vi.fn(({ number }: GitItemRef) =>
			number === 412
				? new Promise<GitItemRef | null>(() => {})
				: Promise.resolve(pr(398, NOW, "closed"))
		);
		const refreshed = refreshGitItems(items, find, NOW);
		expect(find).toHaveBeenCalledTimes(2);
		await vi.advanceTimersByTimeAsync(GIT_REFRESH_TIMEOUT_MS);
		expect(await refreshed).toEqual([items[0], pr(398, NOW, "closed")]);
	});
});
