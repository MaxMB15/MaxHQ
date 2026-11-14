// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Moving and copying whole lines of a note's plain text, the way a code
// editor does with Option and the arrow keys. The lines are every line the
// selection touches, except the last when the selection only reaches its
// start. A line is the text between line breaks, however it wraps on
// screen. Like the list keys, each takes offsets and gives back the
// replacements and the selection after them, so the phone can share it.

import type { ListEdit, OffsetSelection } from "./lists";

/** Up the text, or down it. */
export type LineDirection = -1 | 1;

/** Where the lines the selection touches start and end, the end before its line break. */
const linesTouched = (text: string, selection: OffsetSelection) => {
	const start = Math.min(selection.anchor, selection.head);
	const end = Math.max(selection.anchor, selection.head);
	// A selection that ends at a line's start leaves that line out.
	const last = end > start && text[end - 1] === "\n" ? end - 1 : end;
	const from = start === 0 ? 0 : text.lastIndexOf("\n", start - 1) + 1;
	const found = text.indexOf("\n", last);
	return { from, to: found === -1 ? text.length : found };
};

// The selection moved with its lines. An end at the next line's start, left out
// of them, comes back to their end.
const shifted = (
	selection: OffsetSelection,
	lines: { readonly from: number; readonly to: number },
	by: number
): OffsetSelection => ({
	anchor: Math.min(selection.anchor, lines.to) + by,
	head: Math.min(selection.head, lines.to) + by,
});

/**
 * Swaps the lines the selection touches with the line above or below,
 * keeping the selection on them. At the first or last line nothing moves.
 */
export const moveLines = (
	text: string,
	selection: OffsetSelection,
	direction: LineDirection
): ListEdit => {
	const touched = linesTouched(text, selection);
	const { from, to } = touched;
	const lines = text.slice(from, to);
	if (direction === -1) {
		if (from === 0) return { replacements: [], selection };
		const above = from < 2 ? 0 : text.lastIndexOf("\n", from - 2) + 1;
		const line = text.slice(above, from - 1);
		return {
			replacements: [{ from: above, to, text: `${lines}\n${line}` }],
			selection: shifted(selection, touched, -(line.length + 1)),
		};
	}
	if (to === text.length) return { replacements: [], selection };
	const found = text.indexOf("\n", to + 1);
	const below = found === -1 ? text.length : found;
	const line = text.slice(to + 1, below);
	return {
		replacements: [{ from, to: below, text: `${line}\n${lines}` }],
		selection: shifted(selection, touched, line.length + 1),
	};
};

/**
 * Copies the lines the selection touches above or below themselves, with
 * the selection on the copy that went in: the lower one going down, the
 * upper one going up.
 */
export const copyLines = (
	text: string,
	selection: OffsetSelection,
	direction: LineDirection
): ListEdit => {
	const touched = linesTouched(text, selection);
	const { from, to } = touched;
	const lines = text.slice(from, to);
	return direction === -1
		? { replacements: [{ from, to: from, text: `${lines}\n` }], selection }
		: {
				replacements: [{ from: to, to, text: `\n${lines}` }],
				selection: shifted(selection, touched, lines.length + 1),
			};
};
