// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a PR's Summary says on every client: each reviewer's latest verdict,
// whether it merges cleanly, and what a merge or an approval came to.

import { t } from "../i18n";

import type { PullDetail, PullReview, ReviewVerdict } from "../bindings";

import type { PullActed } from "./act";
import { MERGE_CONFLICT_REASON } from "./merge";

/** Each verdict, in words. */
export const VERDICT: Record<ReviewVerdict, string> = {
	get approved() {
		return t("Approved");
	},
	get changes_requested() {
		return t("Asked for changes");
	},
	get commented() {
		return t("Commented");
	},
	get dismissed() {
		return t("Review dismissed");
	},
};

/**
 * Each reviewer's latest verdict, in the order they first reviewed. A
 * comment after a verdict leaves the verdict standing.
 */
export const latestVerdicts = (reviews: readonly PullReview[]): PullReview[] => {
	const latest = new Map<string, PullReview>();
	for (const review of reviews) {
		const who = review.author.toLowerCase();
		const before = latest.get(who);
		if (review.verdict === "commented" && before && before.verdict !== "commented") continue;
		latest.set(who, review);
	}
	return [...latest.values()];
};

/** Whether the PR merges cleanly, in words. */
export const mergeability = (pr: PullDetail): string | null => {
	const pull = pr.summary.pull;
	if (pull.state !== "open") return null;
	if (pull.mergeable === "conflicting") return MERGE_CONFLICT_REASON;
	if (pull.mergeable === "unknown") return t("GitHub is still checking for conflicts.");
	return t("No conflicts with {{base}}.", { base: pull.base });
};

const ACTED: Record<"merge" | "approve", Record<"done" | "queued", string>> = {
	merge: {
		get done() {
			return t("Merged.");
		},
		get queued() {
			return t("Queued to merge.");
		},
	},
	approve: {
		get done() {
			return t("Approved.");
		},
		get queued() {
			return t("Approved.");
		},
	},
};

/** What a merge or an approval came to, as a note. */
export const actedNote = (
	act: "merge" | "approve",
	acted: PullActed
): { readonly text: string; readonly error: boolean } =>
	acted.state === "refused"
		? { text: acted.reason, error: true }
		: { text: ACTED[act][acted.state], error: false };
