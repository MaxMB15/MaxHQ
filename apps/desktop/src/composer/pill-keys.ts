// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: Left and Right step
// over a pill in one press, a paste that ends with a pill gets a space after
// it, and copy and cut write the pills' own clipboard type next to the plain
// text. Right to left lines and styled Markdown are left out, since
// the composer holds plain text. Backspace and Delete take a pill whole, and
// Option+Backspace takes the word before it too. The space a pill gets
// beside words is the shared one in @maxhq/shared/pills/spacing.

import { DRAFT_CLIPBOARD_MIME, encodeClipboardHtml } from "@maxhq/shared/pills/clipboard";
import { OBJECT_MARK } from "@maxhq/shared/pills/draft";
import { closeHistory } from "@tiptap/pm/history";
import type { Node as ProseNode, ResolvedPos } from "@tiptap/pm/model";
import { type EditorState, TextSelection, type Transaction } from "@tiptap/pm/state";

import { isPill } from "./doc";

/** Which way a key moves: back toward the start, or on toward the end. */
export type Direction = -1 | 1;

const beside = ($pos: ResolvedPos, direction: Direction): ProseNode | null =>
	(direction === -1 ? $pos.nodeBefore : $pos.nodeAfter) ?? null;

/** The caret moved over the pill beside it, or null when there's none. */
export const stepOverPill = (state: EditorState, direction: Direction): Transaction | null => {
	const { selection } = state;
	if (!selection.empty) return null;
	const pill = beside(selection.$from, direction);
	if (!isPill(pill)) return null;
	const to = selection.from + direction * pill.nodeSize;
	return state.tr.setSelection(TextSelection.create(state.doc, to)).scrollIntoView();
};

/** A word and the spaces after it, at the end of the text before a pill. */
const WORD_BEFORE = new RegExp(`[^\\s${OBJECT_MARK}]*\\s*$`, "u");

/**
 * The pill beside the caret taken out whole, or null when there's none.
 * With `word`, the word before the pill goes too. It's an undo step of its
 * own, so undo brings back just the pill.
 */
export const deletePill = (
	state: EditorState,
	direction: Direction,
	word = false
): Transaction | null => {
	const { selection } = state;
	if (!selection.empty) return null;
	const { $from } = selection;
	const pill = beside($from, direction);
	if (!isPill(pill)) return null;
	let from = direction === -1 ? $from.pos - pill.nodeSize : $from.pos;
	const to = from + pill.nodeSize;
	if (word && direction === -1) {
		const offset = $from.parentOffset - pill.nodeSize;
		const before = $from.parent.textBetween(0, offset, undefined, OBJECT_MARK);
		from -= WORD_BEFORE.exec(before)?.[0].length ?? 0;
	}
	return closeHistory(state.tr.delete(from, to)).scrollIntoView();
};

/**
 * Writes a copied piece of a draft: plain text for any app, and the
 * fragment in its own type and in HTML, so another composer keeps the pills.
 */
export const writeDraftClipboard = (
	data: Pick<DataTransfer, "setData">,
	plain: string,
	fragment: string | null
) => {
	data.setData("text/plain", plain);
	if (fragment === null) return;
	data.setData(DRAFT_CLIPBOARD_MIME, fragment);
	data.setData("text/html", encodeClipboardHtml(plain, fragment));
};
