// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: a pill placed in
// the text gets a space where it would run into a word, before it and after
// it, and the caret steps over a space that's already there.

import type { Segment } from "./draft";

/** Whether text ends, or starts, with a space or a line break. */
export const spaceAtEnd = (text: string) => /\s$/u.test(text);
export const spaceAtStart = (text: string) => /^\s/u.test(text);

/**
 * Segments with a space added where a pill would run into the words
 * beside it: before a pill that follows a word, and after one that ends
 * the insert. `before` and `after` are the line's text around the insert.
 * `always` adds the space after any insert, such as a picked `#123`.
 * Returns the segments, and whether the caret should step over a space
 * that was already there.
 */
export const spaced = (
	segments: readonly Segment[],
	before: string,
	after: string,
	always = false
): { segments: Segment[]; skipSpace: boolean } => {
	const out = [...segments];
	if (out[0]?.type === "pill" && before !== "" && !spaceAtEnd(before))
		out.unshift({ type: "text", text: " " });
	const last = out.at(-1);
	const wantsSpace = last?.type === "pill" || (always && last !== undefined);
	if (!wantsSpace) return { segments: out, skipSpace: false };
	if (spaceAtStart(after)) return { segments: out, skipSpace: true };
	out.push({ type: "text", text: " " });
	return { segments: out, skipSpace: false };
};
