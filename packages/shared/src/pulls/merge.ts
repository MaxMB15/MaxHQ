// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: each merge method's
// words, and why a PR can't merge, in the order the reasons are checked.
// The rest is MaxHQ's: the methods are the ones the
// repository allows, and in place of a read-only connection a merge needs
// your GitHub sign-in, on a device that's online.

import { t } from "../i18n";

import type { MergeMethod, PullDetail } from "../bindings";
import { type GithubSignIn, signInReason } from "./devices";

/** Each merge method's title and what it does, in menu order. */
export const MERGE_METHODS = {
	squash: {
		get title() {
			return t("Squash and merge");
		},
		get description() {
			return t("Combine the commits into one commit.");
		},
	},
	merge: {
		get title() {
			return t("Create a merge commit");
		},
		get description() {
			return t("Keep the commits and add a merge commit.");
		},
	},
	rebase: {
		get title() {
			return t("Rebase and merge");
		},
		get description() {
			return t("Add the commits without a merge commit.");
		},
	},
} as const satisfies Record<MergeMethod, { title: string; description: string }>;

/** The merge methods the PR's repository allows, in menu order. */
export const allowedMergeMethods = (pr: PullDetail): MergeMethod[] => {
	const order = Object.keys(MERGE_METHODS) as MergeMethod[];
	return order.filter((method) => pr.merge_methods.includes(method));
};

/** Why a PR with conflicts can't merge. */
export const MERGE_CONFLICT_REASON = "Resolve the branch conflicts before merging.";

/** Why the PR can't be merged from here, or null when it can. */
export const mergeUnavailableReason = (pr: PullDetail, signIn: GithubSignIn): string | null => {
	const pull = pr.summary.pull;
	if (pull.state === "merged") return t("This PR has been merged.");
	if (pull.state === "closed") return t("Reopen this PR before merging.");
	if (signIn.state !== "ready") return signInReason(signIn, "merge");
	if (pull.draft) return t("Mark this draft ready for review before merging.");
	if (pull.mergeable === "conflicting") return MERGE_CONFLICT_REASON;
	if (!allowedMergeMethods(pr).length)
		return t("This repository has no available merge methods.");
	return null;
};
