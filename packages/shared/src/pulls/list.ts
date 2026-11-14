// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the merge readiness
// ranking and the sorts of a PR list: the merge
// readiness tiers and the other sorts. The rest is MaxHQ's: rows are
// `PullSummary`, there are no groups, no "blocked on me" sort and no
// icons, and a PR's number stands in for when it was opened, since the
// list doesn't carry that.

import { t } from "../i18n";

import type { PullSummary } from "../bindings";

/** How a PR list is sorted. */
export type PullSort = "ready" | "updated" | "newest" | "oldest" | "largest" | "smallest";

/** The sorts a PR list offers, in menu order, each with its label. */
export const SORT_OPTIONS = [
	{
		value: "ready",
		get label() {
			return t("Merge readiness");
		},
	},
	{
		value: "updated",
		get label() {
			return t("Recently updated");
		},
	},
	{
		value: "newest",
		get label() {
			return t("Newest shown");
		},
	},
	{
		value: "oldest",
		get label() {
			return t("Oldest shown");
		},
	},
	{
		value: "largest",
		get label() {
			return t("Largest shown");
		},
	},
	{
		value: "smallest",
		get label() {
			return t("Smallest shown");
		},
	},
] as const satisfies ReadonlyArray<{ value: PullSort; label: string }>;

/** Lines added and deleted together. */
const size = (row: PullSummary): number => row.pull.additions + row.pull.deletions;

/** Whether a PR's size is known, which a read that found no lines leaves open. */
const measured = (row: PullSummary): boolean => size(row) > 0;

/**
 * How ready a PR is to merge, from 0 for approved with checks passing,
 * then passing, then the rest, drafts and closed ones, to 4 for conflicts.
 */
const readiness = ({ pull }: PullSummary): number => {
	if (pull.mergeable === "conflicting") return 4;
	if (pull.state !== "open") return 3;
	if (pull.draft) return 2;
	if (pull.checks === "passing" && pull.review === "approved") return 0;
	if (pull.checks === "passing") return 1;
	return 2;
};

/** How many of the PRs are ready to merge: approved, with checks passing. */
export const readyCount = (rows: ReadonlyArray<PullSummary>): number =>
	rows.filter((row) => readiness(row) === 0).length;

/**
 * PRs in the order they can merge, by their readiness. Within a tier,
 * sized PRs come first, smallest first, then the newest change.
 */
export const rankPullRequestsByMergeReadiness = <Row extends PullSummary>(
	rows: ReadonlyArray<Row>,
	hasMeasuredSize: (row: Row) => boolean = measured
): Row[] => {
	return [...rows].sort((left, right) => {
		const byTier = readiness(left) - readiness(right);
		if (byTier !== 0) return byTier;
		const sized = Number(hasMeasuredSize(right)) - Number(hasMeasuredSize(left));
		return sized || size(left) - size(right) || right.pull.updated_at - left.pull.updated_at;
	});
};

/** PRs in the order `sort` asks for. */
export const sortPulls = <Row extends PullSummary>(
	rows: ReadonlyArray<Row>,
	sort: PullSort,
	hasMeasuredSize: (row: Row) => boolean = measured
): Row[] => {
	if (sort === "ready") return rankPullRequestsByMergeReadiness(rows, hasMeasuredSize);
	const updated = (left: Row, right: Row) => right.pull.updated_at - left.pull.updated_at;
	if (sort === "updated") return [...rows].sort(updated);
	return [...rows].sort((left, right) => {
		if (sort === "newest" || sort === "oldest") {
			const opened = left.key.number - right.key.number;
			return (sort === "newest" ? -opened : opened) || updated(left, right);
		}
		const sizedFirst = Number(hasMeasuredSize(right)) - Number(hasMeasuredSize(left));
		const sized = size(left) - size(right);
		return sizedFirst || (sort === "largest" ? -sized : sized) || updated(left, right);
	});
};
