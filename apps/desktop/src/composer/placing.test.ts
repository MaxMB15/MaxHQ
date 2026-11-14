// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { getSchema } from "@tiptap/core";
import Document from "@tiptap/extension-document";
import Paragraph from "@tiptap/extension-paragraph";
import Text from "@tiptap/extension-text";
import { history, undo } from "@tiptap/pm/history";
import { EditorState, NodeSelection, TextSelection } from "@tiptap/pm/state";
import { describe, expect, it } from "vitest";

import { OBJECT_MARK as MARK, type Segment } from "@maxhq/shared/pills/draft";

import { markedTextOf, offsetOf, paragraphsFor, posOf } from "./doc";
import { PILL_EXTENSIONS } from "./pill-nodes";
import { caretPlaced, dropPosition, movePill, placeSegments } from "./placing";

const schema = getSchema([Document, Paragraph, Text, ...PILL_EXTENSIONS]);

const file = (key: string): Segment => ({ type: "pill", pill: { type: "attachment", key } });
const text = (value: string): Segment => ({ type: "text", text: value });

/** A box holding the segments that hasn't had a caret yet. */
const untouched = (segments: Segment[]) =>
	EditorState.create({
		schema,
		doc: schema.node("doc", null, paragraphsFor(schema, segments)),
		plugins: [history(), caretPlaced()],
	});

/** A box holding the segments, with the caret at `caret` in the draft's text. */
const stateOf = (segments: Segment[], caret: number) => {
	const state = untouched(segments);
	const pos = posOf(state.doc, caret);
	return state.apply(state.tr.setSelection(TextSelection.create(state.doc, pos)));
};

/** "Read", a pill and "now please", with the pill at offset 5. */
const READ = [text("Read "), file("k1"), text(" now please")];

/** The text after moving the pill at offset `from` to offset `to`, and the caret's offset. */
const moved = (segments: Segment[], from: number, to: number) => {
	const state = stateOf(segments, 0);
	const tr = movePill(state, posOf(state.doc, from), posOf(state.doc, to));
	if (!tr) return null;
	const next = state.apply(tr);
	return { text: markedTextOf(next.doc), caret: offsetOf(next.doc, next.selection.head) };
};

describe("moving a pill", () => {
	it("takes its space along and fits in at the end", () => {
		expect(moved(READ, 5, 17)).toEqual({
			text: `Read now please ${MARK} `,
			caret: 18,
		});
	});

	it("goes to the start of the line with a space after it", () => {
		expect(moved(READ, 5, 0)).toEqual({ text: `${MARK} Read now please`, caret: 2 });
	});

	it("sits between two words with one space each side", () => {
		// Before "please".
		expect(moved(READ, 5, 11)).toEqual({ text: `Read now ${MARK} please`, caret: 11 });
		// After "now", where the space before "please" serves.
		expect(moved(READ, 5, 10)).toEqual({ text: `Read now ${MARK} please`, caret: 11 });
	});

	it("moves to another line", () => {
		const lines = [text("Read "), file("k1"), text(" now\nthen")];
		expect(moved(lines, 5, 15)).toEqual({ text: `Read now\nthen ${MARK} `, caret: 16 });
	});

	it("leaves the words around it closed up when it brought no space", () => {
		expect(moved([text("ab"), file("k1"), text("cd")], 2, 5)).toEqual({
			text: `abcd ${MARK} `,
			caret: 7,
		});
	});

	it("stays put just before or just after itself, or after its own space", () => {
		expect(moved(READ, 5, 5)).toBeNull();
		expect(moved(READ, 5, 6)).toBeNull();
		expect(moved(READ, 5, 7)).toBeNull();
	});

	it("does nothing without a pill there", () => {
		const state = stateOf(READ, 0);
		expect(movePill(state, 1, 10)).toBeNull();
	});

	it("comes back in one undo step", () => {
		const state = stateOf(READ, 0);
		const next = state.apply(movePill(state, 6, 18)!);
		let back: EditorState | null = null;
		expect(undo(next, (tr) => (back = next.apply(tr)))).toBe(true);
		expect(back!.doc.eq(state.doc)).toBe(true);
	});
});

describe("where a drop goes", () => {
	/** The text and caret's offset after dropping a pill into `state`. */
	const dropped = (state: EditorState) => {
		const at = dropPosition(state);
		const next = state.apply(placeSegments(state.tr, at, at, [file("k2")]));
		return { text: markedTextOf(next.doc), caret: offsetOf(next.doc, next.selection.head) };
	};

	it("goes in at the caret, which ends after it", () => {
		expect(dropped(stateOf([text("Ask now")], 4))).toEqual({
			text: `Ask ${MARK} now`,
			caret: 6,
		});
	});

	it("keeps selected text and goes in after it", () => {
		const state = stateOf([text("Ask now")], 0);
		const selected = state.apply(state.tr.setSelection(TextSelection.create(state.doc, 1, 4)));
		expect(dropped(selected)).toEqual({ text: `Ask ${MARK} now`, caret: 6 });
	});

	it("goes after a selected pill", () => {
		const state = stateOf([text("Ask "), file("k1"), text(" now")], 0);
		const selected = state.apply(state.tr.setSelection(NodeSelection.create(state.doc, 5)));
		expect(dropped(selected)).toEqual({ text: `Ask ${MARK} ${MARK} now`, caret: 8 });
	});

	it("goes at the end of a box that hasn't had a caret", () => {
		const state = untouched([text("Ask now\nthen")]);
		expect(state.selection.from).toBe(1);
		expect(dropped(state)).toEqual({ text: `Ask now\nthen ${MARK} `, caret: 15 });
	});
});
