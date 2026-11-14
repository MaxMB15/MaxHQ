// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a PR chip on an agent shows and offers: the PR's state the way
// GitHub colors it, how its checks stand, and the messages it can send the
// agent about the PR. A link the watcher hasn't read yet shows neutral,
// with no checks. The messages are in
// MaxHQ's words, and are plain messages to the agent, so the phone can
// offer the same ones.

import { t } from "../i18n";

import type { PullLink } from "../bindings";

/** The state a chip shows: GitHub's, or `unread` before the first read. */
export type ChipState = "open" | "draft" | "merged" | "closed" | "unread";

/** How a chip's checks dot stands. */
export type ChipChecks = "passing" | "failing" | "pending" | "unknown";

/** The state a link's chip shows. */
export const chipState = (link: PullLink): ChipState => {
	const pull = link.snapshot;
	if (!pull) return "unread";
	if (pull.state === "open" && pull.draft) return "draft";
	return pull.state;
};

/** The checks a link's chip shows, or null for a PR with none or not read yet. */
export const chipChecks = (link: PullLink): ChipChecks | null => {
	const checks = link.snapshot?.checks;
	return checks === undefined || checks === "none" ? null : checks;
};

const STATE_WORDS: Record<ChipState, string> = {
	open: "open",
	draft: "draft",
	merged: "merged",
	closed: "closed",
	get unread() {
		return t("not read yet");
	},
};

const CHECKS_WORDS: Record<ChipChecks, string> = {
	get passing() {
		return t("checks passing");
	},
	get failing() {
		return t("checks failing");
	},
	get pending() {
		return t("checks running");
	},
	get unknown() {
		return t("checks unknown");
	},
};

/** What a chip says to a screen reader: the PR, its title, its state and its checks. */
export const chipLabel = (link: PullLink): string => {
	const checks = chipChecks(link);
	return [
		t("Pull request {{link_key_number}}", { link_key_number: link.key.number }),
		link.snapshot?.title,
		STATE_WORDS[chipState(link)],
		checks ? CHECKS_WORDS[checks] : undefined,
	]
		.filter(Boolean)
		.join(", ");
};

// Which checks a folded chip shows: the one that needs you most.
const CHECKS_RANK: readonly ChipChecks[] = ["failing", "pending", "unknown", "passing"];

/**
 * One chip standing for several PRs, as a sidebar row shows them: their
 * count, the state they share or null when they differ, the checks of the
 * one that needs you most, and a line per PR for its tooltip.
 */
export type FoldedChip = {
	readonly text: string;
	readonly state: ChipState | null;
	readonly checks: ChipChecks | null;
	readonly title: string;
	readonly label: string;
};

/** Folds several links into one chip. */
export const foldChips = (links: readonly PullLink[]): FoldedChip => {
	const states = new Set(links.map(chipState));
	const checks = new Set(links.map(chipChecks));
	return {
		text: t("{{value1}} PRs", { value1: links.length }),
		state: states.size === 1 ? [...states][0]! : null,
		checks: CHECKS_RANK.find((c) => checks.has(c)) ?? null,
		title: links
			.map((link) => [`#${link.key.number}`, link.snapshot?.title].filter(Boolean).join(" "))
			.join("\n"),
		label: t("{{links_length}} pull requests: {{links_map_chipLabel__join}}", {
			links_length: links.length,
			links_map_chipLabel__join: links.map(chipLabel).join("; "),
		}),
	};
};

/** A message a chip can send the agent about its PR. */
export type PullPrompt = {
	readonly task: "checks" | "conflicts" | "explain";
	/** What the menu calls it. */
	readonly label: string;
	/** The message. */
	readonly text: string;
};

/**
 * The messages that apply to a link's PR: fixing its failing checks and its
 * merge conflicts while it's open and has them, and explaining it always.
 */
export const pullPrompts = (link: PullLink): PullPrompt[] => {
	const { repo, number } = link.key;
	const pr = `pull request #${number} in ${repo}`;
	const branch =
		"First make sure you're on its branch, and check it out in its own worktree if you aren't.";
	const open = link.snapshot?.state === "open";
	const prompts: PullPrompt[] = [];
	if (open && link.snapshot?.checks === "failing") {
		prompts.push({
			task: "checks",
			label: t("Fix the failing checks"),
			text: `Fix the failing checks on ${pr}. ${branch} Read the latest check results, confirm each failure before you change code, keep the fixes focused, and run the checks that failed before you push.`,
		});
	}
	if (open && link.snapshot?.mergeable === "conflicting") {
		prompts.push({
			task: "conflicts",
			label: t("Fix the merge conflicts"),
			text: `Resolve the merge conflicts between ${pr} and its base branch. ${branch} Bring it up to date the way this repository does, keep what both sides meant, and run the relevant checks before you push.`,
		});
	}
	prompts.push({
		task: "explain",
		label: t("Explain the PR"),
		text: `Explain ${pr}. Read its diff and walk me through what it's for, what it changes, its risks, and what I should read closely before approving it. Don't change any code.`,
	});
	return prompts;
};
