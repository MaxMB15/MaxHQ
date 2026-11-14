// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: a quote keeps where
// it starts in its row's text, with runs of whitespace counted as one
// space, and is found again there. The original also keeps the
// text on each side and where the quote ends. MaxHQ keeps just the start,
// which still holds while a row streams, since a row only grows at its
// end. It also trims the whitespace a selection picks up at its ends.

/** A quote and where it starts in its row's normalized text. */
export type QuoteAnchor = {
	readonly text: string;
	readonly position: number;
};

/** Where a quote sits in a row's text, in UTF-16 units. */
export type QuoteSpan = { readonly from: number; readonly to: number };

const normalize = (text: string): string => text.replace(/\s+/g, " ");

/**
 * The quote that runs from `from` to `to` in a row's text, with where it
 * starts. Null when only whitespace is picked.
 */
export const quoteAnchor = (text: string, from: number, to: number): QuoteAnchor | null => {
	let start = Math.max(0, Math.min(from, to));
	let end = Math.min(text.length, Math.max(from, to));
	while (start < end && /\s/.test(text[start]!)) start += 1;
	while (end > start && /\s/.test(text[end - 1]!)) end -= 1;
	if (start === end) return null;
	return { text: text.slice(start, end), position: normalize(text.slice(0, start)).length };
};

/** The offset in `text` of an offset in its normalized form. */
const rawOffset = (text: string, normalizedOffset: number): number => {
	let offset = 0;
	for (const match of text.matchAll(/\s+|\S+/g)) {
		const whitespace = /\s/.test(match[0][0]!);
		const length = whitespace ? 1 : match[0].length;
		if (normalizedOffset <= offset + length) {
			return (
				match.index +
				(whitespace && normalizedOffset > offset
					? match[0].length
					: normalizedOffset - offset)
			);
		}
		offset += length;
	}
	return text.length;
};

/**
 * Where a quote sits in a row's text now: the place it shows up nearest to
 * where it started, which is that place itself unless the row's text has
 * changed before it. Null when it's gone.
 */
export const findQuote = (text: string, anchor: QuoteAnchor): QuoteSpan | null => {
	const normalized = normalize(text);
	const quote = normalize(anchor.text);
	if (quote.trim().length === 0) return null;

	let found: number | null = null;
	for (
		let start = normalized.indexOf(quote);
		start !== -1;
		start = normalized.indexOf(quote, start + 1)
	) {
		if (
			found === null ||
			Math.abs(start - anchor.position) < Math.abs(found - anchor.position)
		) {
			found = start;
		}
		if (start >= anchor.position) break;
	}
	return found === null
		? null
		: { from: rawOffset(text, found), to: rawOffset(text, found + quote.length) };
};
