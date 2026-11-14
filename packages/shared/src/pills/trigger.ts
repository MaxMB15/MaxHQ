// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: finding the menu
// trigger in a draft. The
// `@`, `#` and `$` menus are kept, since MaxHQ has no slash menu yet, and a
// menu never opens inside code. Any currency sign opens the skills menu,
// unless the word is an amount of money or has a second sign, as `$$` and
// `$x^2$` do, which are math. The range covers the whole word, so with the
// caret inside `#123` the menu replaces all of it.

import { codeSpans, inCode } from "./draft";

/**
 * Which menu a word opens: `@` for files, folders and agents, `#` for pull
 * requests and issues, and `$` for skills.
 */
export type TriggerKind = "path" | "git-item" | "skill";

/** The word that opens a menu, and what it asks for. */
export type Trigger = {
	readonly kind: TriggerKind;
	/** What follows the trigger character. */
	readonly query: string;
	/** The word's span, which a pick replaces. */
	readonly rangeStart: number;
	readonly rangeEnd: number;
};

const clampCursor = (text: string, cursor: number): number => {
	if (!Number.isFinite(cursor)) return text.length;
	return Math.max(0, Math.min(text.length, Math.floor(cursor)));
};

/** Whitespace ends a word, and so does a pill's mark. */
const isBreak = (char: string): boolean =>
	char === " " || char === "\n" || char === "\t" || char === "\r" || char === "￼";

const GIT_ITEM_WORD = /^#([\p{L}\p{N}][\p{L}\p{N}_-]*)?$/u;

const SKILL_SIGN = /^\p{Sc}/u;
const ANY_SIGN = /\p{Sc}/u;

/** `$5`, `€9.99` and `$10k` are money, not skills, with the punctuation prose puts after one. */
const isAmount = (query: string): boolean =>
	/^\.?[0-9][0-9,._]*(?:[kKmMbBtT]|[eE][0-9]+)?[.,;:!?)]*$/u.test(query);

/** The menu the word at the caret opens, or null for none. */
export const detectTrigger = (text: string, cursorInput: number): Trigger | null => {
	const cursor = clampCursor(text, cursorInput);
	let start = cursor;
	while (start > 0 && !isBreak(text[start - 1] ?? "")) start -= 1;
	// A pill right before the word means it doesn't start a word.
	if (start > 0 && text[start - 1] === "￼") return null;
	let end = cursor;
	while (end < text.length && !isBreak(text[end] ?? "")) end += 1;
	if (start === cursor) return null;
	if (inCode(codeSpans(text), start + 1)) return null;

	const word = text.slice(start, end);
	const gitItem = GIT_ITEM_WORD.exec(word);
	if (gitItem)
		return { kind: "git-item", query: gitItem[1] ?? "", rangeStart: start, rangeEnd: end };
	const sign = SKILL_SIGN.exec(word);
	if (sign) {
		const query = word.slice(sign[0].length);
		if (isAmount(query) || ANY_SIGN.test(query)) return null;
		return { kind: "skill", query, rangeStart: start, rangeEnd: end };
	}
	if (word.startsWith("@"))
		return { kind: "path", query: word.slice(1), rangeStart: start, rangeEnd: end };
	return null;
};
