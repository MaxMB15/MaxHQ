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

/** How many more patch lines each "Show more" shows. */
export const PATCH_PAGE = 400;

/** The most lines in one chunk of a patch. */
export const PATCH_CHUNK = 200;

/**
 * A patch's lines in chunks to draw and highlight on their own: one for
 * each hunk, its header first, and a long hunk cut every `most` lines. A
 * hunk is one stretch of each file, so highlighting it alone loses nothing.
 */
export const patchChunks = (
	lines: readonly PatchLine[],
	most: number = PATCH_CHUNK
): PatchLine[][] => {
	const chunks: PatchLine[][] = [];
	let chunk: PatchLine[] = [];
	for (const line of lines) {
		if (chunk.length > 0 && (line.type === "hunk" || chunk.length >= most)) {
			chunks.push(chunk);
			chunk = [];
		}
		chunk.push(line);
	}
	if (chunk.length > 0) chunks.push(chunk);
	return chunks;
};

/** One file of a change, ready to draw. */
export type PatchFile = {
	readonly change: FileEdit;
	/** Its lines in chunks, or null when the CLI sent no diff for it. */
	readonly chunks: readonly (readonly PatchLine[])[] | null;
};

/** Each file of a change, its patch read into chunks. */
export const patchFiles = (changes: readonly FileEdit[]): PatchFile[] =>
	changes.map((change) => ({
		change,
		chunks: change.patch === null ? null : patchChunks(patchLines(change.patch)),
	}));

/** What a change shows: its files, whether the cap left anything out, and how many lines. */
export type ShownPatch = {
	readonly files: readonly PatchFile[];
	readonly more: boolean;
	/** The patch lines left out. */
	readonly hidden: number;
};

const lineCount = (file: PatchFile): number =>
	file.chunks?.reduce((sum, chunk) => sum + chunk.length, 0) ?? 0;

/**
 * The files of a change and their chunks, up to `limit` lines in all. The
 * files past the limit are left out, and the chunk the limit falls in is
 * cut. Every chunk shown whole is the one `files` holds, so a view drawn
 * from it stays as it was when more shows. With no limit it shows everything.
 */
export const shownPatch = (
	files: readonly PatchFile[],
	limit: number | null = PATCH_LINES
): ShownPatch => {
	if (limit === null) return { files, more: false, hidden: 0 };
	const shown: PatchFile[] = [];
	let left = limit;
	for (const [at, file] of files.entries()) {
		const count = lineCount(file);
		if (left <= 0 || count > left) {
			const rest = files.slice(at).reduce((sum, next) => sum + lineCount(next), 0);
			if (left > 0) shown.push({ ...file, chunks: cut(file.chunks ?? [], left) });
			return { files: shown, more: true, hidden: rest - Math.max(0, left) };
		}
		shown.push(file);
		left -= count;
	}
	return { files: shown, more: false, hidden: 0 };
};

// The first `count` lines of `chunks`, the chunks shown whole as they are.
const cut = (
	chunks: readonly (readonly PatchLine[])[],
	count: number
): (readonly PatchLine[])[] => {
	const kept: (readonly PatchLine[])[] = [];
	let left = count;
	for (const chunk of chunks) {
		if (left <= 0) break;
		kept.push(chunk.length > left ? chunk.slice(0, left) : chunk);
		left -= chunk.length;
	}
	return kept;
};

/** Where a patch line's colours come from: a line of one side's code. */
export type PatchSpot = { readonly side: "old" | "new"; readonly line: number };

/**
 * A chunk of a patch as the code of each side, to highlight as a whole: the
 * old side's context and removed lines, and the new side's context and
 * added lines, each null when it has none. `spots` says where each patch
 * line's colours come from, null for a hunk header. A context line takes
 * the new side's.
 */
export type PatchSides = {
	readonly old: string | null;
	readonly new: string | null;
	readonly spots: readonly (PatchSpot | null)[];
};

/** A chunk of a patch as the code of each side. */
export const patchSides = (lines: readonly PatchLine[]): PatchSides => {
	const before: string[] = [];
	const after: string[] = [];
	const spots: (PatchSpot | null)[] = [];
	for (const line of lines) {
		// A lone carriage return would split a line in two.
		const text = line.text.includes("\r") ? line.text.replaceAll("\r", "") : line.text;
		if (line.type === "hunk") {
			spots.push(null);
		} else if (line.type === "remove") {
			spots.push({ side: "old", line: before.length });
			before.push(text);
		} else {
			spots.push({ side: "new", line: after.length });
			if (line.type === "context") before.push(text);
			after.push(text);
		}
	}
	return {
		old: before.length > 0 ? before.join("\n") : null,
		new: after.length > 0 ? after.join("\n") : null,
		spots,
	};
};

/** A run of lines of one kind in a chunk, from `start` up to `end`. */
export type PatchRun = {
	readonly type: PatchLine["type"];
	readonly start: number;
	readonly end: number;
};

/** A chunk's lines in runs of one kind, each at most `most` lines. */
export const patchRuns = (lines: readonly PatchLine[], most: number): PatchRun[] => {
	const runs: PatchRun[] = [];
	let start = 0;
	for (let at = 1; at <= lines.length; at++) {
		const type = lines[start]!.type;
		if (at === lines.length || lines[at]!.type !== type || at - start >= most) {
			runs.push({ type, start, end: at });
			start = at;
		}
	}
	return runs;
};
