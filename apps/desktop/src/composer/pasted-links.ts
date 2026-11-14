// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A pasted link to a pull request or issue waits as text while its lookup
// runs. This keeps where each one is as the text changes around it, and
// forgets one once anything changes inside it, so a pill only ever takes
// the place of the link as it was pasted.

import { gitNumbers } from "@maxhq/shared/pills/draft";
import { Extension } from "@tiptap/core";
import { closeHistory } from "@tiptap/pm/history";
import { type EditorState, Plugin, PluginKey, type Transaction } from "@tiptap/pm/state";

import { markedTextOf, offsetOf } from "./doc";

/** Where a pasted link is, and its text. */
type Waiting = { readonly from: number; readonly to: number; readonly text: string };

type Links = ReadonlyMap<number, Waiting>;

type Change = { readonly add: Waiting & { readonly id: number } } | { readonly drop: number };

const PASTED_LINKS = new PluginKey<Links>("pastedLinks");

/** Where a link is after `tr`, or null once `tr` changed anything inside it. */
const mapped = (tr: Transaction, waiting: Waiting): Waiting | null => {
	let { from, to } = waiting;
	for (const map of tr.mapping.maps) {
		let touched = false;
		map.forEach((start, end) => {
			if (start < to && end > from) touched = true;
		});
		if (touched) return null;
		from = map.map(from, 1);
		to = map.map(to, -1);
	}
	return { ...waiting, from, to };
};

/** Keeps track of the links pasted into the text box until each one's lookup answers. */
export const PastedLinks = Extension.create({
	name: "pastedLinks",
	addProseMirrorPlugins: () => [
		new Plugin<Links>({
			key: PASTED_LINKS,
			state: {
				init: () => new Map(),
				apply: (tr, old) => {
					const change = tr.getMeta(PASTED_LINKS) as Change | undefined;
					if (!tr.docChanged && !change) return old;
					const next = new Map<number, Waiting>();
					for (const [id, waiting] of old) {
						const now = tr.docChanged ? mapped(tr, waiting) : waiting;
						if (now) next.set(id, now);
					}
					if (change && "add" in change) {
						const { id, ...waiting } = change.add;
						next.set(id, waiting);
					}
					if (change && "drop" in change) next.delete(change.drop);
					return next;
				},
			},
		}),
	],
});

let lastId = 0;

/**
 * Types a link at the caret and keeps track of it. Gives the link's ID, or
 * null when it didn't go in as typed.
 */
export const insertLink = (
	state: EditorState,
	dispatch: (tr: Transaction) => void,
	text: string
): number | null => {
	const tr = state.tr.insertText(text).scrollIntoView();
	const to = tr.selection.from;
	const from = to - text.length;
	if (from < 0 || tr.doc.textBetween(from, to) !== text) {
		dispatch(tr);
		return null;
	}
	lastId += 1;
	const id = lastId;
	dispatch(tr.setMeta(PASTED_LINKS, { add: { id, from, to, text } } satisfies Change));
	return id;
};

/**
 * Puts `text` in place of a pasted link, if the link is still there as it
 * was pasted and `text` reads there as one `#123` or `owner/repo#123`.
 * Either way the link isn't kept track of after. Null just forgets it.
 * Returns whether the text went in.
 */
export const swapLink = (
	state: EditorState,
	dispatch: (tr: Transaction) => void,
	id: number,
	text: string | null
): boolean => {
	const waiting = PASTED_LINKS.getState(state)?.get(id);
	if (!waiting) return false;
	const drop: Change = { drop: id };
	const tr =
		text !== null && state.doc.textBetween(waiting.from, waiting.to) === waiting.text
			? state.tr.replaceWith(waiting.from, waiting.to, state.schema.text(text))
			: null;
	const at = tr ? offsetOf(tr.doc, waiting.from) : -1;
	const reads =
		tr !== null &&
		text !== null &&
		gitNumbers(markedTextOf(tr.doc)).some(
			(span) => span.from === at && span.to === at + text.length
		);
	if (!tr || !reads) {
		dispatch(state.tr.setMeta(PASTED_LINKS, drop).setMeta("addToHistory", false));
		return false;
	}
	tr.setMeta(PASTED_LINKS, drop);
	dispatch(closeHistory(tr));
	return true;
};
