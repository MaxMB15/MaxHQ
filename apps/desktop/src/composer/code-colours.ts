// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A fenced code block that names a language the chat can highlight takes the
// chat's colours, from the same worker and its cache. markdown.ts finds the
// fences, and this colours their code with decorations of its own, so it
// never changes their boxes, their folded lines or what the text is.
//
// Highlighting is async. A fence keeps the colours it last had while the
// worker works on its new text, moved along with the text, so typing never
// flashes it plain. Only a fence whose language or text changed asks again,
// one request at a time, the fence with the caret first. A reply for text
// that has changed since is dropped, and the next request goes out.

import { Extension } from "@tiptap/core";
import { type EditorState, Plugin, PluginKey, type Transaction } from "@tiptap/pm/state";
import { Decoration, DecorationSet } from "@tiptap/pm/view";

import {
	cachedLines,
	highlight,
	type Language,
	languageOf,
	type Lines,
	MAX_HIGHLIGHT,
} from "@/agent-view/highlight";

import { codeFences, MAX_BLOCK_LENGTH } from "./markdown";

/** Where colours come from, which tests replace. */
export type CodeColoursOptions = {
	/** Highlights code, or gives null when it can't. */
	readonly highlight: (code: string, lang: Language) => Promise<Lines | null>;
	/** Lines already highlighted for this code, if any. */
	readonly cached: (code: string, lang: Language) => Lines | undefined;
};

/** A fence's code, from where its first line starts through where its last one ends. */
type Code = {
	readonly from: number;
	readonly to: number;
	readonly lang: Language;
	readonly text: string;
};

/** A reply from the highlighter, for the code it was asked about. */
type Reply = { readonly lang: Language; readonly text: string; readonly lines: Lines };

type Colours = {
	readonly decorations: DecorationSet;
	/** Each fence's range, with the code its colours on screen were worked out for. */
	readonly painted: readonly Code[];
	/** The fences whose colours don't match their code yet. */
	readonly wanted: readonly Code[];
};

const COLOURS = new PluginKey<Colours>("codeColours");

/** The code of every fence that can take colours, in the order of the text. */
const fencesOf = (state: EditorState): Code[] => {
	const found: Code[] = [];
	for (const fence of codeFences(state)) {
		const lang = languageOf(fence.lang);
		const { text } = fence;
		if (!lang || text.length > MAX_HIGHLIGHT || text.length > MAX_BLOCK_LENGTH) continue;
		// Each line break is two positions, between paragraphs.
		let lines = 0;
		for (let at = text.indexOf("\n"); at !== -1; at = text.indexOf("\n", at + 1)) lines++;
		found.push({ from: fence.from, to: fence.from + text.length + lines, lang, text });
	}
	return found;
};

const sameCode = (a: Code | Reply, b: Code | Reply): boolean =>
	a.lang === b.lang && a.text === b.text;

const overlaps = (a: Code, b: Code): boolean => a.from <= b.to && b.from <= a.to;

/** The decorations that colour a fence's code, or null when the lines don't match it. */
const paint = (code: Code, lines: Lines): Decoration[] | null => {
	if (lines.length !== code.text.split("\n").length) return null;
	const out: Decoration[] = [];
	let pos = code.from;
	for (const line of lines) {
		for (const token of line) {
			const to = pos + token.content.length;
			if (token.color && to > pos) {
				let style = `color: ${token.color}`;
				if (token.fontStyle && token.fontStyle & 1) style += "; font-style: italic";
				if (token.fontStyle && token.fontStyle & 2) style += "; font-weight: 600";
				out.push(Decoration.inline(pos, to, { style }));
			}
			pos = to;
		}
		pos += 2;
	}
	return pos - 2 === code.to ? out : null;
};

/**
 * Matches the colours on screen to the fences there are now. A fence whose
 * code has colours, cached or in `reply`, takes them. One that has none yet
 * keeps the colours it had, and a fence that's gone loses its colours.
 */
const settle = (
	tr: Transaction | null,
	state: EditorState,
	old: Colours,
	options: CodeColoursOptions,
	reply: Reply | undefined
): Colours => {
	const { doc } = state;
	const moved = tr?.docChanged ? tr.mapping : null;
	let decorations = moved ? old.decorations.map(moved, doc) : old.decorations;
	const before = moved
		? old.painted
				.map((each) => ({
					...each,
					from: moved.map(each.from, -1),
					to: moved.map(each.to, 1),
				}))
				.filter((each) => each.from < each.to)
		: old.painted;
	const fences = fencesOf(state);
	const clear = (range: Code) => {
		const found = decorations.find(range.from, range.to);
		if (found.length > 0) decorations = decorations.remove(found);
	};
	const painted: Code[] = [];
	const wanted: Code[] = [];
	let next = 0;
	for (const fence of fences) {
		// The colours on screen in this fence's place, which ones before it no longer have.
		while (next < before.length && before[next]!.to < fence.from) clear(before[next++]!);
		const had = next < before.length && overlaps(before[next]!, fence) ? before[next] : null;
		if (had) next++;
		if (had && sameCode(had, fence) && had.from === fence.from) {
			painted.push(fence);
			continue;
		}
		const lines =
			reply && sameCode(reply, fence) ? reply.lines : options.cached(fence.text, fence.lang);
		const drawn = lines ? paint(fence, lines) : null;
		if (drawn) {
			if (had) clear(had);
			clear(fence);
			decorations = decorations.add(doc, drawn);
			painted.push(fence);
		} else {
			// The last colours stay until the new ones come, but only inside the fence.
			if (had) {
				if (had.from < fence.from) clear({ ...had, to: fence.from - 1 });
				if (had.to > fence.to) clear({ ...had, from: fence.to + 1 });
				painted.push({ ...had, from: fence.from, to: fence.to });
			}
			if (!lines) wanted.push(fence);
		}
	}
	while (next < before.length) clear(before[next++]!);
	return { decorations, painted, wanted };
};

const EMPTY: Colours = { decorations: DecorationSet.empty, painted: [], wanted: [] };

/** Colours the code in fenced code blocks, as the chat does. */
export const CodeColours = Extension.create<CodeColoursOptions>({
	name: "codeColours",
	// It reads the fences from the look's new state, so its plugin has to come
	// after the look's, which a lower priority does in Tiptap.
	priority: 99,
	addOptions: () => ({ highlight, cached: cachedLines }),
	addProseMirrorPlugins() {
		const options = this.options;
		return [
			new Plugin<Colours>({
				key: COLOURS,
				state: {
					init: (_, state) => settle(null, state, EMPTY, options, undefined),
					apply: (tr, old, _, state) => {
						const reply = tr.getMeta(COLOURS) as Reply | undefined;
						if (!tr.docChanged && !reply) return old;
						return settle(tr, state, old, options, reply);
					},
				},
				props: { decorations: (state) => COLOURS.getState(state)?.decorations ?? null },
				view: (view) => {
					let asking = false;
					// Code the highlighter couldn't do, which stays plain and isn't asked about again.
					const failed = new Set<string>();
					const keyOf = (code: Code) => `${code.lang}\n${code.text}`;
					// Sends the next fence that wants colours, the one with the caret first.
					const ask = () => {
						if (asking || view.isDestroyed) return;
						const wanted = (COLOURS.getState(view.state)?.wanted ?? []).filter(
							(each) => !failed.has(keyOf(each))
						);
						if (wanted.length === 0) return;
						const { head } = view.state.selection;
						const code =
							wanted.find((each) => head >= each.from && head <= each.to) ??
							wanted[0]!;
						asking = true;
						void options
							.highlight(code.text, code.lang)
							.catch(() => null)
							.then((lines) => {
								asking = false;
								if (view.isDestroyed) return;
								if (!lines) {
									if (failed.size >= 500) failed.clear();
									failed.add(keyOf(code));
								} else if (
									COLOURS.getState(view.state)?.wanted.some((each) =>
										sameCode(each, code)
									)
								) {
									const reply: Reply = {
										lang: code.lang,
										text: code.text,
										lines,
									};
									view.dispatch(
										view.state.tr
											.setMeta(COLOURS, reply)
											.setMeta("addToHistory", false)
									);
								}
								ask();
							});
					};
					ask();
					return { update: ask };
				},
			}),
		];
	},
});
