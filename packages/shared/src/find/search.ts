// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: how find matches
// text, which match an ask makes active, and the items around it. maxd
// matches and selects the same way, so a row's count here equals its count
// there. The rest is MaxHQ's: the query loses its outer whitespace, the
// text counts each run of whitespace as one space, as a quote does, and a
// match never crosses a line.

import type { FindAt, FindItem, TimelineFind } from "../bindings";

/** The longest query find takes, as maxd checks it. */
export const MAX_FIND_QUERY = 200;
/** How many items with matches an answer names around the active one. */
export const NEAR_ITEMS = 17;

/**
 * Lowercases text one UTF-16 unit for one, so offsets in the folded text
 * are offsets in the text. A character whose lowercase is longer stays as
 * it is, and a final sigma folds to the other sigma.
 */
export const foldFindCase = (text: string): string => {
	const lowered = text.toLowerCase().replace(/ς/g, "σ");
	if (lowered.length === text.length) return lowered;
	let folded = "";
	for (const character of text) {
		const lower = character.toLowerCase().replace(/ς/g, "σ");
		folded += lower.length === character.length ? lower : character;
	}
	return folded;
};

/** The query as find looks for it, without its outer whitespace. */
export const findQueryOf = (typed: string): string => typed.trim();

/** Where each match of `query` starts in `text`, ignoring case. Matches don't overlap. */
export const findOccurrences = (text: string, query: string): number[] => {
	const offsets: number[] = [];
	if (text.length === 0 || query.length === 0) return offsets;
	const folded = foldFindCase(text);
	const needle = foldFindCase(query);
	let cursor = 0;
	while (cursor <= folded.length - needle.length) {
		const offset = folded.indexOf(needle, cursor);
		if (offset === -1) break;
		offsets.push(offset);
		cursor = offset + needle.length;
	}
	return offsets;
};

/** How many times `query` shows in `text`, ignoring case. */
export const countOccurrences = (text: string, query: string): number =>
	findOccurrences(text, query).length;

/** Text with each run of whitespace as one space, as find reads a block. */
export const collapseSpace = (text: string): string => text.replace(/\s+/g, " ");

/** One line of text as find reads it. */
export type FindLine = {
	readonly text: string;
	/** For each character of `text`, its index in the text it was read from. */
	readonly at: readonly number[];
};

const SPACE = /\s/;

/**
 * Reads a stretch of text as find does: a new line at each line break,
 * each run of whitespace as one space, and without a line's outer
 * whitespace. A match never crosses a line.
 */
export const findLines = (raw: string): FindLine[] => {
	const lines: FindLine[] = [];
	let text = "";
	let at: number[] = [];
	// Where the run of whitespace being read started, and whether it breaks the line.
	let run = -1;
	let breaks = false;
	const end = () => {
		if (text.length > 0) lines.push({ text, at });
		text = "";
		at = [];
	};
	for (let i = 0; i < raw.length; i++) {
		const character = raw[i]!;
		if (SPACE.test(character)) {
			if (run < 0) {
				run = i;
				breaks = false;
			}
			if (character === "\n") breaks = true;
			continue;
		}
		if (run >= 0) {
			if (breaks) end();
			else if (text.length > 0) {
				text += " ";
				at.push(run);
			}
			run = -1;
		}
		text += character;
		at.push(i);
	}
	end();
	return lines;
};

/** The index `delta` steps from `index` among `total` matches, wrapping round. */
export const stepFindIndex = (index: number, total: number, delta: number): number => {
	if (total <= 0) return 0;
	const held = Math.min(Math.max(0, Math.trunc(index)), total - 1);
	return (((held + delta) % total) + total) % total;
};

/** The item in `near` that holds the match at `index`, if it's there. */
export const nearItemAt = (near: readonly FindItem[], index: number): FindItem | undefined =>
	near.find((entry) => index >= entry.first && index < entry.first + entry.count);

/** One item's matches, in timeline order, for {@link selectFind}. */
export type FindCount = { readonly item: string; readonly count: number };

/**
 * Which match `at` makes active among items counted in timeline order, and
 * the items around it. This is maxd's selection, kept here for the sample
 * data and the tests.
 */
export const selectFind = (counts: readonly FindCount[], at: FindAt): TimelineFind => {
	const total = counts.reduce((sum, entry) => sum + entry.count, 0);
	let active: number;
	if (at.type === "index") active = Math.min(Math.max(0, at.index), Math.max(0, total - 1));
	else {
		let requested = 0;
		const start = counts.findIndex((entry) => entry.item === at.from.item);
		if (start >= 0) {
			for (const entry of counts.slice(0, start)) requested += entry.count;
			requested += Math.min(at.from.occurrence, counts[start]?.count ?? 0);
			if (requested >= total) requested = 0;
		}
		active = total > 0 ? (((requested + at.offset) % total) + total) % total : 0;
	}

	const entries: FindItem[] = [];
	let first = 0;
	for (const { item, count } of counts) {
		if (count > 0) entries.push({ item, first, count });
		first += count;
	}
	const holder = entries.findIndex(
		(entry) => active >= entry.first && active < entry.first + entry.count
	);
	const near =
		entries.length <= NEAR_ITEMS
			? entries
			: Array.from(
					{ length: NEAR_ITEMS },
					(_, i) =>
						entries[(holder + i - (NEAR_ITEMS >> 1) + entries.length) % entries.length]!
				);
	const found = entries[holder];
	return {
		total,
		active,
		found: found ? { item: found.item, occurrence: active - found.first } : null,
		near,
	};
};
