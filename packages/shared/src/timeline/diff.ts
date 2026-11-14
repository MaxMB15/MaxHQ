// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A file change's patch as lines to draw without a diff library. Patches
// from maxd are unified diffs, except that Claude's planned edits and
// Codex's added or deleted files come as bare "+" and "-" lines. A patch
// with no hunk header reads as one hunk without line numbers.

import { t } from "../i18n";
import type { FileEdit } from "../bindings";

/** One line of a patch. */
export type PatchLine = {
	readonly type: "hunk" | "add" | "remove" | "context";
	/**
	 * The line's number in the new file, or in the old one for a removed
	 * line. Null for a hunk header, and when the patch gives no numbers.
	 */
	readonly number: number | null;
	/** The line without its sign, or the whole header for a hunk. */
	readonly text: string;
};

const HUNK = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/;

/** Reads a patch into its lines, leaving out the file headers. */
export const patchLines = (patch: string): PatchLine[] => {
	const all = patch.split("\n");
	const numbered = all.some((line) => HUNK.test(line));
	const lines: PatchLine[] = [];
	let oldLine = 0;
	let newLine = 0;
	let inHunk = !numbered;
	for (const line of all) {
		const hunk = HUNK.exec(line);
		if (hunk) {
			lines.push({ type: "hunk", number: null, text: line });
			oldLine = Number(hunk[1]);
			newLine = Number(hunk[2]);
			inHunk = true;
			continue;
		}
		if (line.startsWith("diff --git")) {
			inHunk = false;
			continue;
		}
		if (!inHunk || line.startsWith("\\")) continue;
		if (line.startsWith("+")) {
			lines.push({ type: "add", number: numbered ? newLine++ : null, text: line.slice(1) });
		} else if (line.startsWith("-")) {
			lines.push({
				type: "remove",
				number: numbered ? oldLine++ : null,
				text: line.slice(1),
			});
		} else if (line.startsWith(" ")) {
			lines.push({
				type: "context",
				number: numbered ? newLine++ : null,
				text: line.slice(1),
			});
			oldLine++;
		}
	}
	return lines;
};

/** The file a change is for, as "old → new" for a rename. */
export const editPath = (change: Pick<FileEdit, "op" | "path" | "old_path">): string =>
	change.op === "rename" && change.old_path ? `${change.old_path} → ${change.path}` : change.path;

/**
 * The counts a change shows: what it adds, unless it only removes, and
 * what it removes when it removes anything. Null for a count left out.
 */
export const shownCounts = (
	additions: number,
	deletions: number
): { readonly additions: number | null; readonly deletions: number | null } => ({
	additions: additions > 0 || deletions === 0 ? additions : null,
	deletions: deletions > 0 ? deletions : null,
});

/** The counts in words for a screen reader, such as "12 added, 3 removed". */
export const countsLabel = (additions: number, deletions: number): string => {
	const shown = shownCounts(additions, deletions);
	const parts = [
		shown.additions !== null ? t("{{value1}} added", { value1: shown.additions }) : null,
		shown.deletions !== null ? t("{{value1}} removed", { value1: shown.deletions }) : null,
	];
	return parts.filter((part) => part !== null).join(", ");
};

/** The lines a change adds and removes across its files. */
export const changeTotals = (
	changes: readonly FileEdit[]
): { readonly additions: number; readonly deletions: number } => ({
	additions: changes.reduce((sum, change) => sum + change.additions, 0),
	deletions: changes.reduce((sum, change) => sum + change.deletions, 0),
});

/** How many patch lines a feed shows before "Show all", about a phone screen. */
export const PATCH_LINES = 40;

/** One file of a change, ready to draw. */
export type PatchFile = {
	readonly change: FileEdit;
	/** Its lines, or null when the CLI sent no diff for it. */
	readonly lines: readonly PatchLine[] | null;
};

/** What a change shows: its files, and whether the cap left anything out. */
export type ShownPatch = {
	readonly files: readonly PatchFile[];
	readonly more: boolean;
};

/**
 * The files of a change and their lines, up to `limit` lines in all. The
 * files past the limit are left out. With no limit it shows everything.
 */
export const shownPatch = (
	changes: readonly FileEdit[],
	limit: number | null = PATCH_LINES
): ShownPatch => {
	const files: PatchFile[] = [];
	let left = limit ?? Infinity;
	for (const change of changes) {
		if (left <= 0) return { files, more: true };
		const lines = change.patch === null ? null : patchLines(change.patch);
		if (lines && lines.length > left) {
			files.push({ change, lines: lines.slice(0, left) });
			return { files, more: true };
		}
		files.push({ change, lines });
		left -= lines?.length ?? 0;
	}
	return { files, more: false };
};
