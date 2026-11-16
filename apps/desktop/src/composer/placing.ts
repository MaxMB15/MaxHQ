// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Where text and pills go in the text box. A drop goes in at the caret the
// box kept when the pointer left it, not under the pointer, since the drop
// overlay covers the text while a drag is over it. A box that hasn't had a
// caret yet takes the drop at the end. A pill dragged to a new place in the
// text moves there with the space it brought, as one undo step.

import { OBJECT_MARK, type Segment } from "@maxhq/shared/pills/draft";
import { spaced } from "@maxhq/shared/pills/spacing";
import { Extension } from "@tiptap/core";
import { closeHistory } from "@tiptap/pm/history";
import { Fragment, type Node as ProseNode, Slice } from "@tiptap/pm/model";
import {
	type EditorState,
	Plugin,
	PluginKey,
	Selection,
	TextSelection,
	type Transaction,
} from "@tiptap/pm/state";

import { paragraphsFor, pillOf } from "./doc";

/**
 * Puts text and pills between two positions of the transaction's document,
 * with a space where a pill would run into a word, and leaves the caret
 * after them. `always` adds the space after any insert.
 */
export const placeSegments = (
	tr: Transaction,
	from: number,
	to: number,
	segments: readonly Segment[],
	always = false
): Transaction => {
	const $from = tr.doc.resolve(from);
	const $to = tr.doc.resolve(to);
	const before = $from.parent.textBetween(0, $from.parentOffset, undefined, OBJECT_MARK);
	const after = $to.parent.textBetween(
		$to.parentOffset,
		$to.parent.content.size,
		undefined,
		OBJECT_MARK
	);
	const fitted = spaced(segments, before, after, always);
	const slice = new Slice(
		Fragment.from(paragraphsFor(tr.doc.type.schema, fitted.segments)),
		1,
		1
	);
	const steps = tr.steps.length;
	tr.replace(from, to, slice);
	const end = tr.mapping.slice(steps).map(to, 1) + (fitted.skipSpace ? 1 : 0);
	return tr.setSelection(TextSelection.create(tr.doc, Math.min(end, tr.doc.content.size)));
};

const PLACED = new PluginKey<boolean>("caretPlaced");

/**
 * Notes whether the box has had a caret since it was made or emptied: any
 * change to its text or its selection gives it one.
 */
export const caretPlaced = (): Plugin<boolean> =>
	new Plugin<boolean>({
		key: PLACED,
		state: {
			init: () => false,
			apply: (tr, placed) => placed || tr.selectionSet || tr.docChanged,
		},
	});

/** The plugin above, for the editor. */
export const CaretPlaced = Extension.create({
	name: "caretPlaced",
	addProseMirrorPlugins: () => [caretPlaced()],
});

/**
 * Where a drop goes in: at the caret, or after the selection so the
 * selected text stays. A box that hasn't had a caret takes it at the end.
 */
export const dropPosition = (state: EditorState): number => {
	if (PLACED.getState(state) !== true) return TextSelection.atEnd(state.doc).from;
	return Selection.near(state.doc.resolve(state.selection.to), -1).from;
};

/**
 * The range a pill takes up with the space after it, when that space came
 * with it: it's there, more follows on the line, and taking it doesn't
 * close up two words.
 */
export const pillWithSpace = (doc: ProseNode, pos: number): { from: number; to: number } => {
	const $pos = doc.resolve(pos);
	const index = $pos.index();
	const end = pos + (doc.nodeAt(pos)?.nodeSize ?? 0);
	const before = index === 0 ? "" : doc.textBetween(pos - 1, pos, undefined, OBJECT_MARK);
	const after = doc.textBetween(end, Math.min(end + 1, doc.content.size), undefined, OBJECT_MARK);
	const withSpace = after === " " && index < $pos.parent.childCount - 1 && /^\s?$/.test(before);
	return { from: pos, to: withSpace ? end + 1 : end };
};

/**
 * The pill at `from` moved to `to`, a position in the same document, with
 * the space it brought and the spaces its new place needs. The caret ends
 * after it, and undo puts it back in one step. Null when there's no pill
 * at `from`, or the pill would land where it is.
 */
export const movePill = (state: EditorState, from: number, to: number): Transaction | null => {
	const node = state.doc.nodeAt(from);
	const pill = node ? pillOf(node) : null;
	if (!node || !pill) return null;
	if (to === from || to === from + node.nodeSize) return null;
	const taken = pillWithSpace(state.doc, from);
	const tr = state.tr.delete(taken.from, taken.to);
	const at = tr.mapping.map(to);
	placeSegments(tr, at, at, [{ type: "pill", pill }]);
	if (tr.doc.eq(state.doc)) return null;
	return closeHistory(tr);
};
