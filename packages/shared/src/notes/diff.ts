// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What changed between two versions of a note's text, as replacements in the
// older one. Lines are compared first, which is quick on a long note, and
// then the characters inside each run of changed lines, so a word fixed in a
// paragraph replaces that word and not the whole paragraph.

import { diffChars, diffLines } from "diff";

/** Text that takes the place of `before.slice(from, to)`. */
export type Replacement = {
	readonly from: number;
	readonly to: number;
	readonly text: string;
};

/**
 * Past this many characters on both sides, a run of changed lines goes in
 * whole, since comparing its characters would take too long.
 */
const MAX_CHARS = 20_000;

/** The characters that differ between a run of old lines and the new ones, from `at`. */
const charReplacements = (removed: string, added: string, at: number): Replacement[] => {
	if (removed === "" || added === "" || removed.length + added.length > MAX_CHARS)
		return [{ from: at, to: at + removed.length, text: added }];
	const out: Replacement[] = [];
	let offset = at;
	for (const change of diffChars(removed, added)) {
		if (change.added) {
			const last = out.at(-1);
			if (last && last.to === offset)
				out[out.length - 1] = { ...last, text: last.text + change.value };
			else out.push({ from: offset, to: offset, text: change.value });
		} else if (change.removed) {
			out.push({ from: offset, to: offset + change.value.length, text: "" });
			offset += change.value.length;
		} else offset += change.value.length;
	}
	return out;
};

/**
 * The replacements that turn `before` into `after`, in order and apart from
 * each other, with offsets in `before`.
 */
export const replacementsBetween = (before: string, after: string): Replacement[] => {
	if (before === after) return [];
	const out: Replacement[] = [];
	let offset = 0;
	let removed = "";
	let added = "";
	const flush = () => {
		if (removed !== "" || added !== "") out.push(...charReplacements(removed, added, offset));
		offset += removed.length;
		removed = "";
		added = "";
	};
	for (const change of diffLines(before, after)) {
		if (change.removed) removed += change.value;
		else if (change.added) added += change.value;
		else {
			flush();
			offset += change.value.length;
		}
	}
	flush();
	return out;
};

/** `before` with the replacements made. */
export const applyReplacements = (before: string, replacements: readonly Replacement[]): string => {
	let out = "";
	let copied = 0;
	for (const { from, to, text } of replacements) {
		out += before.slice(copied, from) + text;
		copied = to;
	}
	return out + before.slice(copied);
};
