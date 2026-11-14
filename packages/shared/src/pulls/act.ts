// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Merging and approving as you. Both go to the device holding your GitHub
// sign-in with the head the viewer showed, so a push in between is refused
// instead of merged. Once a merge lands, each device running an agent
// linked to the PR is told to read the repository again now, so its agent
// settles without waiting for the next pass.

import { t } from "../i18n";

import type { MergeMethod, PullDetail, PullKey, Timestamp } from "../bindings";
import type { ActionResult, DataSource } from "../data/source";
import { errorText } from "../timeline/store";
import type { ViewState } from "../view/replica";
import { githubSignIn, type GithubSignIn, signInReason } from "./devices";
import { devicesWithPull } from "./links";

/** What a merge or an approval came to, for the viewer to show. */
export type PullActed =
	| { readonly state: "done" }
	/** GitHub took the merge but the PR is still open, as in a merge queue. */
	| { readonly state: "queued" }
	| { readonly state: "refused"; readonly reason: string };

/** The PR a merge or an approval is for, at the head the viewer showed. */
export type PullAt = { readonly key: PullKey; readonly headSha: string };

/** Whether `login`'s latest verdict on the PR approves its current head. */
export const approvedByYou = (pr: PullDetail, login: string): boolean => {
	const yours = pr.reviews.filter(
		(review) =>
			review.verdict !== "commented" && review.author.toLowerCase() === login.toLowerCase()
	);
	const last = yours[yours.length - 1];
	return last?.verdict === "approved" && last.commit_sha === pr.summary.pull.head_sha;
};

/** Why you can't approve the PR from here, or null when you can. */
export const approveUnavailableReason = (pr: PullDetail, signIn: GithubSignIn): string | null => {
	const pull = pr.summary.pull;
	if (pull.state !== "open") return t("This PR isn't open anymore.");
	if (signIn.state !== "ready") return signInReason(signIn, "approve");
	if (pull.author.toLowerCase() === signIn.login.toLowerCase()) {
		return t("GitHub doesn't let you approve your own PR.");
	}
	if (approvedByYou(pr, signIn.login)) return t("You already approved this commit.");
	return null;
};

/** A result the device refused, or one that didn't arrive, in words. */
const refusal = (result: ActionResult): PullActed | undefined => {
	if (result.status === "error") return { state: "refused", reason: errorText(result.error) };
	if (result.data.status === "refused") return { state: "refused", reason: result.data.reason };
	return undefined;
};

/**
 * Merges the PR as you with `method`. Once it's merged, and not only
 * queued, each device with an agent linked to it reads the repository
 * again now.
 */
export const mergePull = async (
	source: Pick<DataSource, "pullAction" | "pullsDue">,
	view: ViewState,
	now: Timestamp,
	pull: PullAt,
	method: MergeMethod
): Promise<PullActed> => {
	const reason = signInReason(githubSignIn(view, now), "merge");
	if (reason) return { state: "refused", reason };
	const { repo, number } = pull.key;
	const result = await source.pullAction(repo, number, { action: "merge", method }, pull.headSha);
	const refused = refusal(result);
	if (refused) return refused;
	if (result.status === "ok" && result.data.status === "pull_merged" && result.data.queued) {
		return { state: "queued" };
	}
	await Promise.all(
		devicesWithPull(view, pull.key).map((device) => source.pullsDue(device, repo))
	);
	return { state: "done" };
};

/** Approves the PR's head as you. */
export const approvePull = async (
	source: Pick<DataSource, "pullAction">,
	view: ViewState,
	now: Timestamp,
	pull: PullAt
): Promise<PullActed> => {
	const reason = signInReason(githubSignIn(view, now), "approve");
	if (reason) return { state: "refused", reason };
	const { repo, number } = pull.key;
	const result = await source.pullAction(repo, number, { action: "approve" }, pull.headSha);
	return refusal(result) ?? { state: "done" };
};
