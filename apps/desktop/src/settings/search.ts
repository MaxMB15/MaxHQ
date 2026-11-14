// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Finds the settings pages a search names. Each page has a title and a
// static list of the words on it: its headings and its controls' labels. A
// page matches when every word you typed is in its title or in one of
// those, ignoring case and accents, and the matches come back as spans to
// highlight.

import { fold, hasWord } from "@maxhq/shared/lib/words";

/** A page as the search sees it. */
export type Searchable = {
	readonly title: string;
	/** The page's description, headings and control labels. */
	readonly terms: readonly string[];
};

/** A piece of text, and whether it's part of a match. */
export type Span = { readonly text: string; readonly hit: boolean };

/** What a page shows for a search: its title in spans, and the first other text that matched. */
export type Found = {
	readonly title: readonly Span[];
	readonly term: readonly Span[] | null;
};

/** The text in spans, with each place a word is found marked. */
export const highlight = (text: string, words: readonly string[]): readonly Span[] => {
	const { folded, from } = fold(text);
	const marked = new Array<boolean>(text.length).fill(false);
	for (const word of words) {
		for (let at = folded.indexOf(word); at !== -1; at = folded.indexOf(word, at + 1)) {
			const start = from[at]!;
			const last = from[at + word.length - 1]!;
			const end = last + (text.codePointAt(last)! > 0xffff ? 2 : 1);
			marked.fill(true, start, end);
		}
	}
	const spans: Span[] = [];
	for (let at = 0; at < text.length; at++) {
		const hit = marked[at]!;
		const last = spans.at(-1);
		if (last && last.hit === hit) spans[spans.length - 1] = { text: last.text + text[at], hit };
		else spans.push({ text: text[at]!, hit });
	}
	return spans;
};

// About how many characters of a line the list shows before it cuts it off.
const ROOM = 28;

/**
 * The text, or its end from the word where the first match starts, behind
 * an ellipsis, when that match would sit past what the list shows.
 */
export const excerpt = (text: string, words: readonly string[]): string => {
	const { folded, from } = fold(text);
	let start = -1;
	let end = -1;
	for (const word of words) {
		const at = folded.indexOf(word);
		if (at === -1 || (start !== -1 && from[at]! >= start)) continue;
		start = from[at]!;
		end = from[at + word.length - 1]! + 1;
	}
	if (start === -1 || end <= ROOM) return text;
	const cut = text.lastIndexOf(" ", start) + 1;
	return cut === 0 ? text : `…${text.slice(cut)}`;
};

/**
 * What a page shows for a search, or null when it doesn't match. With no
 * words, every page matches and nothing is marked. The text under the
 * title is the shortest that has every word the title lacks, or else the
 * shortest that has any of them, since a label says more than a sentence.
 */
export const findIn = (page: Searchable, words: readonly string[]): Found | null => {
	if (words.length === 0) return { title: [{ text: page.title, hit: false }], term: null };
	const missing = words.filter((word) => !hasWord(page.title, word));
	if (!missing.every((word) => page.terms.some((term) => hasWord(term, word)))) return null;
	if (missing.length === 0) return { title: highlight(page.title, words), term: null };
	const all = page.terms.filter((t) => missing.every((word) => hasWord(t, word)));
	const some = page.terms.filter((t) => missing.some((word) => hasWord(t, word)));
	const term = (all.length > 0 ? all : some).reduce((a, b) => (b.length < a.length ? b : a));
	return { title: highlight(page.title, words), term: highlight(excerpt(term, words), words) };
};
