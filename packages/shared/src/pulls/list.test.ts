// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { MIN, NOW } from "../test/build";
import { summary } from "../test/pulls";

import { rankPullRequestsByMergeReadiness, readyCount, SORT_OPTIONS, sortPulls } from "./list";

const numbers = (rows: { key: { number: number } }[]) => rows.map((row) => row.key.number);

describe("rankPullRequestsByMergeReadiness", () => {
	it("puts approved and passing first, then passing, then the rest, conflicts last", () => {
		const rows = [
			summary(1, { mergeable: "conflicting", review: "approved" }),
			summary(2, { checks: "failing" }),
			summary(3, { review: "approved" }),
			summary(4),
			summary(5, { draft: true, review: "approved" }),
			summary(6, { state: "merged" }),
		];
		expect(numbers(rankPullRequestsByMergeReadiness(rows))).toEqual([3, 4, 2, 5, 6, 1]);
	});

	it("puts sized PRs first in a tier, smallest first, then the newest change", () => {
		const rows = [
			summary(1, { additions: 0, deletions: 0 }),
			summary(2, { additions: 40 }),
			summary(3, { additions: 4, updated_at: NOW - 5 * MIN }),
			summary(4, { additions: 4, updated_at: NOW - MIN }),
		];
		expect(numbers(rankPullRequestsByMergeReadiness(rows))).toEqual([4, 3, 2, 1]);
	});

	it("takes the caller's word for which sizes are known", () => {
		const rows = [summary(1, { additions: 1 }), summary(2, { additions: 90 })];
		const ranked = rankPullRequestsByMergeReadiness(rows, (row) => row.key.number === 2);
		expect(numbers(ranked)).toEqual([2, 1]);
	});

	it("leaves the rows it was given alone", () => {
		const rows = [summary(1, { checks: "failing" }), summary(2)];
		rankPullRequestsByMergeReadiness(rows);
		expect(numbers(rows)).toEqual([1, 2]);
	});
});

describe("sortPulls", () => {
	const rows = [
		summary(10, { additions: 50, updated_at: NOW - 3 * MIN }),
		summary(12, { additions: 0, deletions: 0, updated_at: NOW - MIN }),
		summary(11, { additions: 5, updated_at: NOW - 2 * MIN, checks: "failing" }),
	];

	it("ranks by merge readiness", () => {
		expect(numbers(sortPulls(rows, "ready"))).toEqual([10, 12, 11]);
	});

	it("sorts by the latest change", () => {
		expect(numbers(sortPulls(rows, "updated"))).toEqual([12, 11, 10]);
	});

	it("sorts by number for newest and oldest", () => {
		expect(numbers(sortPulls(rows, "newest"))).toEqual([12, 11, 10]);
		expect(numbers(sortPulls(rows, "oldest"))).toEqual([10, 11, 12]);
	});

	it("sorts by size with unknown sizes last", () => {
		expect(numbers(sortPulls(rows, "largest"))).toEqual([10, 11, 12]);
		expect(numbers(sortPulls(rows, "smallest"))).toEqual([11, 10, 12]);
	});

	it("offers each sort once, merge readiness first", () => {
		const values = SORT_OPTIONS.map((option) => option.value);
		expect(values[0]).toBe("ready");
		expect(new Set(values).size).toBe(6);
	});
});

describe("readyCount", () => {
	it("counts the open PRs approved with checks passing", () => {
		const rows = [
			summary(1, { review: "approved" }),
			summary(2, { review: "approved", additions: 1, deletions: 0 }),
			summary(3, { review: "approved", additions: 0, deletions: 0 }),
			summary(4),
			summary(5, { additions: 1, deletions: 0 }),
			summary(6, { review: "approved", checks: "pending" }),
			summary(7, { review: "approved", draft: true }),
			summary(8, { review: "approved", mergeable: "conflicting" }),
			summary(9, { review: "approved", state: "merged" }),
		];
		expect(readyCount(rows)).toBe(3);
	});

	it("is zero with none ready", () => {
		expect(readyCount([])).toBe(0);
		expect(readyCount([summary(1)])).toBe(0);
	});
});
