// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The word matching every search box shares: a search splits into words
// without case or accents, and a text holds a word when its folded form
// does.

/**
 * Each character of `text` without case or accents, and where in `text`
 * each one came from.
 */
export const fold = (text: string): { folded: string; from: number[] } => {
	let folded = "";
	const from: number[] = [];
	let at = 0;
	for (const char of text) {
		const plain = char.normalize("NFKD").replace(/\p{M}/gu, "").toLowerCase();
		for (const part of plain) {
			folded += part;
			from.push(at);
		}
		at += char.length;
	}
	return { folded, from };
};

/** The words of a search, without case or accents. */
export const searchWords = (query: string): readonly string[] =>
	fold(query)
		.folded.split(/\s+/)
		.filter((word) => word !== "");

/** Whether the text holds the word, ignoring case and accents. */
export const hasWord = (text: string, word: string): boolean => fold(text).folded.includes(word);

/** Whether the text holds every word, ignoring case and accents. */
export const hasWords = (text: string, words: readonly string[]): boolean => {
	const { folded } = fold(text);
	return words.every((word) => folded.includes(word));
};
