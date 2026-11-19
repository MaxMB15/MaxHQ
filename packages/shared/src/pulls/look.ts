// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The words a PR shows in, the same on every client: its state with the
// tone of its badge, how its checks and reviews stand, how each file
// changed, and how each check went. Each client draws the icons and colours
// itself. A changed file also reads as a file edit, so the phone's diff
// view can show it.

import { t } from "../i18n";

import type {
	CheckState,
	ChecksState,
	FileEdit,
	PullFile,
	PullFileStatus,
	PullSnapshot,
	ReviewState,
} from "../bindings";
import type { Tone } from "../tasks/format";

/** The state a PR's badge shows: GitHub's, with a draft apart. */
export type PullStateKey = "open" | "draft" | "merged" | "closed";

/** The badge's state for `pull`. */
export const pullStateKey = (pull: Pick<PullSnapshot, "state" | "draft">): PullStateKey =>
	pull.state === "open" && pull.draft ? "draft" : pull.state;

/** Each state's words and the tone of its badge. */
export const PULL_STATE: Record<PullStateKey, { readonly label: string; readonly tone: Tone }> = {
	open: {
		get label() {
			return t("Open");
		},
		tone: "good",
	},
	draft: {
		get label() {
			return t("Draft");
		},
		tone: "plain",
	},
	merged: {
		get label() {
			return t("Merged");
		},
		tone: "info",
	},
	closed: {
		get label() {
			return t("Closed");
		},
		tone: "bad",
	},
};

const CHECKS: Record<ChecksState, string> = {
	get passing() {
		return t("Checks passing");
	},
	get failing() {
		return t("Checks failing");
	},
	get pending() {
		return t("Checks running");
	},
	get none() {
		return t("No checks");
	},
	get unknown() {
		return t("Checks unknown");
	},
};

/** The words for how the PR's checks stand. */
export const checksLabel = (state: ChecksState): string => CHECKS[state];

const REVIEW: Record<ReviewState, string> = {
	get approved() {
		return t("Approved");
	},
	get changes_requested() {
		return t("Changes requested");
	},
	get none() {
		return t("No review yet");
	},
};

/** The words for where the PR's reviews leave it. */
export const reviewLabel = (state: ReviewState): string => REVIEW[state];

/** How each file changed, in words. */
export const FILE_STATUS: Record<PullFileStatus, string> = {
	get added() {
		return t("Added");
	},
	get removed() {
		return t("Deleted");
	},
	get modified() {
		return t("Changed");
	},
	get renamed() {
		return t("Renamed");
	},
	get copied() {
		return t("Copied");
	},
	get changed() {
		return t("Mode changed");
	},
	get unchanged() {
		return t("Unchanged");
	},
};

/** The groups the Checks tab shows, the ones that need you first. */
export const CHECK_GROUPS: readonly { readonly state: CheckState; readonly title: string }[] = [
	{
		state: "failing",
		get title() {
			return t("Failing");
		},
	},
	{
		state: "pending",
		get title() {
			return t("Pending");
		},
	},
	{
		state: "passing",
		get title() {
			return t("Passing");
		},
	},
	{
		state: "skipped",
		get title() {
			return t("Skipped");
		},
	},
];

/** How one check went, in words. */
export const CHECK_STATE: Record<CheckState, string> = {
	get failing() {
		return t("Failed");
	},
	get pending() {
		return t("Running");
	},
	get passing() {
		return t("Passed");
	},
	get skipped() {
		return t("Skipped");
	},
};

const FILE_OP: Record<PullFileStatus, FileEdit["op"]> = {
	added: "add",
	removed: "delete",
	modified: "modify",
	renamed: "rename",
	copied: "add",
	changed: "modify",
	unchanged: "modify",
};

/** A PR's changed file as a file edit, which a diff view reads. */
export const pullFileEdit = (file: PullFile): FileEdit => ({
	path: file.path,
	old_path: null,
	op: FILE_OP[file.status],
	additions: file.additions,
	deletions: file.deletions,
	patch: file.patch,
});
