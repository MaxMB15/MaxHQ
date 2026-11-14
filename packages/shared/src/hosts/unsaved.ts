// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A container workspace's unsaved work, as the guest last read it. A
// stopped workspace can't read it again, so its summary is marked old.

import { t } from "../i18n";

import type { Timestamp, UnsavedSummary, Workspace } from "../bindings";
import { formatWhen, listInWords, plural } from "../status/format";

/** A workspace's unsaved work in words. */
export type UnsavedView = {
	/** Whether there's anything to lose. */
	readonly any: boolean;
	/** How many things there are to lose, for a badge. */
	readonly count: number;
	/** Such as "3 changed files and 1 unpushed commit", or "Nothing unsaved". */
	readonly text: string;
	/** Whether it's from before the workspace stopped. */
	readonly old: boolean;
	/** Such as "Read at 09:00". */
	readonly readAt: string;
};

/** How many things a summary counts to lose. */
const countOf = (summary: UnsavedSummary): number =>
	summary.changed + summary.commits + summary.no_upstream;

/** Whether a summary counts anything to lose. */
const anyUnsaved = (summary: UnsavedSummary): boolean => countOf(summary) > 0;

/** The summary's parts in words. */
export const unsavedText = (summary: UnsavedSummary): string => {
	const parts = [
		summary.changed > 0 ? plural(summary.changed, "changed file") : null,
		summary.commits > 0 ? plural(summary.commits, "unpushed commit") : null,
		summary.no_upstream > 0
			? plural(summary.no_upstream, "branch with no upstream", "branches with no upstream")
			: null,
	].filter((p) => p !== null);
	return parts.length === 0 ? t("Nothing unsaved") : listInWords(parts);
};

/** A workspace's unsaved work, or none when its guest hasn't read it. */
export const unsavedOf = (workspace: Workspace, now: Timestamp): UnsavedView | null => {
	const summary = workspace.managed?.unsaved ?? null;
	if (!summary) return null;
	const old = workspace.managed?.state.state === "stopped";
	return {
		any: anyUnsaved(summary),
		count: countOf(summary),
		text: unsavedText(summary),
		old,
		readAt: t("{{value1}} at {{value2}}", {
			value1: old ? t("Last read") : t("Read"),
			value2: formatWhen(summary.read_at, now),
		}),
	};
};

/**
 * What a roll back would lose, for the confirm to show before it sends
 * `discardUnsaved`, or null when it loses nothing. A restore as a new
 * workspace never asks. With no summary from the guest yet the device
 * can't tell, so it asks too.
 */
export const rollbackLoses = (workspace: Workspace, asNew: boolean): string | null => {
	if (asNew) return null;
	const summary = workspace.managed?.unsaved ?? null;
	if (!summary)
		return t("MaxHQ hasn't heard about unsaved work in it yet, so some could be lost");
	return anyUnsaved(summary) ? unsavedText(summary) : null;
};
