// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Spell check that follows macOS. The editor turns the webview's own
// checker off, picks out the prose in each line, asks the native checker
// about lines it hasn't seen, and underlines what comes back with the CSS
// Custom Highlight API, which draws without touching the editor's DOM. A
// right click on an underline opens the native spelling menu.
//
// The editors on one native checker share the lines checked before and the
// words ignored or learned, so Ignore Spelling in one takes the word away
// in all of them. A word stays unmarked while the caret is in it, the way
// macOS leaves a word you're still typing.
//
// A long document checks only the lines on screen and a margin around
// them, again after a pause in scrolling it. Words found before on lines
// past the margin keep their underlines. The check turns off and on with
// the Check spelling setting, without the editor starting over, and an
// editor you can't type in checks nothing.

import { Extension } from "@tiptap/core";
import type { Node as ProseNode } from "@tiptap/pm/model";
import { type EditorState, Plugin, PluginKey } from "@tiptap/pm/state";
import type { EditorView } from "@tiptap/pm/view";

import {
	browserNative,
	type ComposerNative,
	type Misspelling,
	type SpellingChoice,
} from "./native";
import { proseLines } from "./prose";

/** How long typing has to pause before a line is checked again. */
export const SPELL_DELAY_MS = 300;

/** The name the underlines go under in `CSS.highlights`, which the stylesheet styles. */
export const HIGHLIGHT_NAME = "misspelled";

// Lines checked before, which the editors on one checker share. Past this
// many it starts over.
const CACHE_LIMIT = 1000;

/** Lines in a document past which only the ones on screen are checked. */
export const LONG_LINES = 200;

/** Lines checked past each edge of the screen in a long document. */
export const MARGIN_LINES = 60;

const WORD_CHAR = /[\p{L}\p{M}\p{N}'’]/u;

/** A misspelled word at document positions. */
export type Found = {
	readonly from: number;
	readonly to: number;
	readonly misspelling: Misspelling;
};

/** Line indexes, `to` past the last. */
export type LineSpan = { readonly from: number; readonly to: number };

type SpellState = { readonly on: boolean; readonly found: readonly Found[] };

type SpellMeta = { readonly found: readonly Found[] } | { readonly on: boolean };

const spellingKey = new PluginKey<SpellState>("spelling");

/** The misspelled words the editor underlines now. */
export const misspellingsOf = (state: EditorState): readonly Found[] =>
	spellingKey.getState(state)?.found ?? [];

const spellingOn = (state: EditorState): boolean => spellingKey.getState(state)?.on ?? false;

/** Turns the editor's spell check on or off. Off takes the underlines away. */
export const switchSpelling = (view: EditorView, on: boolean): void => {
	if (!spellingKey.getState(view.state) || spellingOn(view.state) === on) return;
	view.dispatch(
		view.state.tr
			.setMeta(spellingKey, { on } satisfies SpellMeta)
			.setMeta("addToHistory", false)
	);
};

/** The lines on screen, from the editor's place in the window, or null when it isn't laid out. */
export const linesOnScreen = (view: EditorView): LineSpan | null => {
	const rect = view.dom.getBoundingClientRect();
	const top = Math.max(rect.top, 0);
	const bottom = Math.min(rect.bottom, window.innerHeight);
	if (rect.width === 0 || bottom <= top) return null;
	const left = rect.left + rect.width / 2;
	const first = view.posAtCoords({ left, top: top + 1 });
	const last = view.posAtCoords({ left, top: bottom - 1 });
	if (!first || !last) return null;
	const { doc } = view.state;
	return { from: doc.resolve(first.pos).index(0), to: doc.resolve(last.pos).index(0) + 1 };
};

/** The lines to check: all of a short document, or the ones on screen and a margin. */
const spanToCheck = (view: EditorView, shown: (view: EditorView) => LineSpan | null): LineSpan => {
	const { doc, selection } = view.state;
	const count = doc.childCount;
	if (count <= LONG_LINES) return { from: 0, to: count };
	// Off screen, as in a hidden tab, the caret's line stands in.
	const caretLine = doc.resolve(selection.head).index(0);
	const span = shown(view) ?? { from: caretLine, to: caretLine + 1 };
	return {
		from: Math.max(0, span.from - MARGIN_LINES),
		to: Math.min(count, span.to + MARGIN_LINES),
	};
};

/** Each paragraph's text and where it starts. */
const linesOf = (doc: ProseNode): { pos: number; text: string }[] => {
	const lines: { pos: number; text: string }[] = [];
	doc.forEach((node, offset) => lines.push({ pos: offset + 1, text: node.textContent }));
	return lines;
};

/** Whether two lists mark the same words in the same places. */
const sameFound = (one: readonly Found[], other: readonly Found[]): boolean =>
	one.length === other.length &&
	one.every(({ from, to, misspelling }, index) => {
		const twin = other[index]!;
		return (
			from === twin.from &&
			to === twin.to &&
			misspelling.word === twin.misspelling.word &&
			misspelling.language === twin.misspelling.language
		);
	});

/** What the editors on one native checker share. */
type Shared = {
	/** Lines checked before, by their prose. */
	readonly cache: Map<string, readonly Misspelling[]>;
	/** Words ignored or learned since the app started. */
	readonly ignored: Set<string>;
	/** Each open editor's way to drop a word and check again. */
	readonly editors: Set<(word: string) => void>;
};

// Keyed by the checking call, which stays the same when Tiptap copies an
// extension's options, as it does a plain object.
const shares = new WeakMap<ComposerNative["checkSpelling"], Shared>();

const sharedBy = (native: ComposerNative): Shared => {
	let shared = shares.get(native.checkSpelling);
	if (!shared) {
		shared = { cache: new Map(), ignored: new Set(), editors: new Set() };
		shares.set(native.checkSpelling, shared);
	}
	return shared;
};

/** Whether a found word still reads the same, with no letter grown onto either end. */
const stillThere = (doc: ProseNode, from: number, to: number, word: string): boolean => {
	if (to <= from || doc.textBetween(from, to) !== word) return false;
	const before = from > 0 ? doc.textBetween(from - 1, from) : "";
	const after = to < doc.content.size ? doc.textBetween(to, to + 1) : "";
	return !WORD_CHAR.test(before) && !WORD_CHAR.test(after);
};

/** The shared highlight every composer adds its underlines to, or null where the webview has none. */
const sharedHighlight = (): Highlight | null => {
	if (typeof CSS === "undefined" || !("highlights" in CSS) || typeof Highlight === "undefined") {
		return null;
	}
	let highlight = CSS.highlights.get(HIGHLIGHT_NAME);
	if (!highlight) {
		highlight = new Highlight();
		CSS.highlights.set(HIGHLIGHT_NAME, highlight);
	}
	return highlight;
};

const under = (range: Range, x: number, y: number): boolean =>
	Array.from(range.getClientRects()).some(
		(rect) => x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom
	);

/** Checks the editor's text and keeps its underlines drawn. */
const spellView = (
	view: EditorView,
	native: ComposerNative,
	delay: number,
	shown: (view: EditorView) => LineSpan | null
) => {
	const { cache, ignored, editors } = sharedBy(native);
	let painted: { range: Range; found: Found }[] = [];
	let timer: ReturnType<typeof setTimeout> | null = null;
	// Whether the check waiting on the timer is due now, which a change
	// doesn't put off, so a note that loads its text right after it opens
	// gets its underlines at once.
	let due = false;
	// The word the caret was in at the last check, left out until the caret leaves it.
	let skipped: { from: number; to: number } | null = null;
	let destroyed = false;

	/** Whether it checks: on, in an editor you can type in. */
	const active = (): boolean => spellingOn(view.state) && view.editable;
	let checking = active();

	const caret = (): number | null => {
		const { selection } = view.state;
		return view.hasFocus() && selection.empty ? selection.from : null;
	};

	const publish = (found: readonly Found[]) => {
		if (sameFound(found, misspellingsOf(view.state))) return;
		view.dispatch(
			view.state.tr
				.setMeta(spellingKey, { found } satisfies SpellMeta)
				.setMeta("addToHistory", false)
		);
	};

	const check = async (): Promise<void> => {
		if (!active()) return;
		const doc = view.state.doc;
		const lines = linesOf(doc);
		const span = spanToCheck(view, shown);
		const prose = proseLines(
			lines.map((line) => line.text),
			span.from,
			span.to
		);
		const known = new Map<string, readonly Misspelling[]>();
		const fresh: string[] = [];
		for (const line of prose) {
			if (line.trim() === "" || known.has(line) || fresh.includes(line)) continue;
			const hit = cache.get(line);
			if (hit) known.set(line, hit);
			else fresh.push(line);
		}
		if (fresh.length > 0) {
			let found: Misspelling[];
			try {
				found = await native.checkSpelling(fresh.join("\n"));
			} catch {
				return;
			}
			if (destroyed) return;
			const starts: number[] = [];
			let at = 0;
			for (const line of fresh) {
				starts.push(at);
				at += line.length + 1;
			}
			const perLine = fresh.map((): Misspelling[] => []);
			for (const misspelling of found) {
				let index = starts.length - 1;
				while (index > 0 && starts[index]! > misspelling.start) index--;
				const offset = starts[index]!;
				perLine[index]!.push({
					...misspelling,
					start: misspelling.start - offset,
					end: misspelling.end - offset,
				});
			}
			if (cache.size + fresh.length > CACHE_LIMIT) cache.clear();
			fresh.forEach((line, index) => {
				cache.set(line, perLine[index]!);
				known.set(line, perLine[index]!);
			});
		}
		if (destroyed || view.state.doc !== doc || !active()) return;

		// Words found before on lines past the margin stay, so scrolling
		// back to them shows them at once.
		const before = lines[span.from]!.pos;
		const after = lines[span.to]?.pos ?? Infinity;
		const kept = misspellingsOf(view.state);
		const at = caret();
		const result: Found[] = kept.filter((found) => found.from < before);
		skipped = null;
		for (let index = span.from; index < span.to; index++) {
			const line = lines[index]!;
			for (const misspelling of known.get(prose[index]!) ?? []) {
				if (ignored.has(misspelling.word)) continue;
				const from = line.pos + misspelling.start;
				const to = line.pos + misspelling.end;
				if (at !== null && from <= at && at <= to) {
					skipped = { from, to };
					continue;
				}
				result.push({ from, to, misspelling });
			}
		}
		result.push(...kept.filter((found) => found.from >= after));
		publish(result);
	};

	const stop = () => {
		if (timer !== null) clearTimeout(timer);
		timer = null;
		due = false;
	};

	const schedule = (ms: number) => {
		if (timer !== null && due) return;
		stop();
		due = ms === 0;
		timer = setTimeout(() => {
			timer = null;
			due = false;
			void check();
		}, ms);
	};

	const paint = () => {
		const highlight = sharedHighlight();
		if (!highlight) return;
		for (const { range } of painted) highlight.delete(range);
		painted = [];
		if (!view.editable) return;
		for (const found of misspellingsOf(view.state)) {
			try {
				const start = view.domAtPos(found.from, 1);
				const end = view.domAtPos(found.to, -1);
				const range = document.createRange();
				range.setStart(start.node, start.offset);
				range.setEnd(end.node, end.offset);
				highlight.add(range);
				painted.push({ range, found });
			} catch {
				// A position the view can't map yet gets drawn on the next update.
			}
		}
	};

	/** Drops a word ignored or learned in any editor, and checks again without it. */
	const drop = (word: string) => {
		if (destroyed) return;
		publish(misspellingsOf(view.state).filter((found) => found.misspelling.word !== word));
		schedule(0);
	};
	editors.add(drop);

	const choose = (choice: SpellingChoice, target: Found) => {
		if (destroyed) return;
		if (choice.kind === "replace") {
			const current = misspellingsOf(view.state).find(
				(found) => found.misspelling === target.misspelling
			);
			if (!current || !view.editable) return;
			const { from, to, misspelling } = current;
			if (!stillThere(view.state.doc, from, to, misspelling.word)) return;
			view.dispatch(view.state.tr.insertText(choice.word, from, to));
			view.focus();
		} else if (choice.kind === "ignored" || choice.kind === "learned") {
			const { word } = target.misspelling;
			ignored.add(word);
			cache.clear();
			for (const each of editors) each(word);
		}
	};

	// The native menu stands in for the webview's everywhere in the text,
	// with guesses on an underline and only Cut, Copy and Paste elsewhere.
	// Off, the webview's own menu shows.
	const onContextMenu = (event: MouseEvent) => {
		if (!active()) return;
		event.preventDefault();
		const hit = painted.find(({ range }) => under(range, event.clientX, event.clientY));
		native
			.spellingMenu(hit?.found.misspelling ?? null, { x: event.clientX, y: event.clientY })
			.then((choice) => {
				if (hit) choose(choice, hit.found);
			}, console.error);
	};

	const onBlur = () => {
		if (skipped !== null) schedule(0);
	};

	// Scrolling a long document brings other lines on screen. Another
	// pane's scrolling, such as a feed following its agent, doesn't, and
	// would put the check off for as long as it went on.
	const onScroll = (event: Event) => {
		if (event.target instanceof Node && !event.target.contains(view.dom)) return;
		if (view.state.doc.childCount > LONG_LINES && active()) schedule(delay);
	};

	view.dom.addEventListener("contextmenu", onContextMenu);
	view.dom.addEventListener("blur", onBlur);
	document.addEventListener("scroll", onScroll, { capture: true, passive: true });
	window.addEventListener("resize", onScroll);
	if (checking) schedule(0);

	return {
		update: (next: EditorView, previous: EditorState) => {
			const now = active();
			if (now !== checking) {
				checking = now;
				if (now) schedule(0);
				else stop();
			} else if (now && next.state.doc !== previous.doc) {
				schedule(delay);
			} else if (now && skipped !== null && !next.state.selection.eq(previous.selection)) {
				const at = caret();
				if (at === null || at < skipped.from || at > skipped.to) schedule(0);
			}
			paint();
		},
		destroy: () => {
			destroyed = true;
			stop();
			editors.delete(drop);
			view.dom.removeEventListener("contextmenu", onContextMenu);
			view.dom.removeEventListener("blur", onBlur);
			document.removeEventListener("scroll", onScroll, { capture: true });
			window.removeEventListener("resize", onScroll);
			const highlight = sharedHighlight();
			for (const { range } of painted) highlight?.delete(range);
			painted = [];
		},
	};
};

/** How `Spelling` runs. */
export type SpellingOptions = {
	readonly native: ComposerNative;
	/** How long typing or scrolling has to pause before a check. */
	readonly delay: number;
	/** Whether it starts on. `switchSpelling` changes it later. */
	readonly on: boolean;
	/** The lines on screen, which a long document checks with a margin. */
	readonly shown: (view: EditorView) => LineSpan | null;
};

/** The ProseMirror plugin behind `Spelling`. */
export const spellingPlugin = ({ native, delay, on, shown }: SpellingOptions) =>
	new Plugin<SpellState>({
		key: spellingKey,
		state: {
			init: () => ({ on, found: [] }),
			apply: (tr, state) => {
				const meta = tr.getMeta(spellingKey) as SpellMeta | undefined;
				if (meta && "on" in meta) return { on: meta.on, found: [] };
				if (meta) return state.on ? { ...state, found: meta.found } : state;
				if (!tr.docChanged || state.found.length === 0) return state;
				const kept: Found[] = [];
				for (const { from, to, misspelling } of state.found) {
					const start = tr.mapping.map(from, 1);
					const end = tr.mapping.map(to, -1);
					if (stillThere(tr.doc, start, end, misspelling.word)) {
						kept.push({ from: start, to: end, misspelling });
					}
				}
				return { ...state, found: kept };
			},
		},
		view: (view) => spellView(view, native, delay, shown),
	});

/** Native spell check for the editor, with the underlines and the right-click menu. */
export const Spelling = Extension.create<SpellingOptions>({
	name: "spelling",
	addOptions() {
		return { native: browserNative, delay: SPELL_DELAY_MS, on: true, shown: linesOnScreen };
	},
	addProseMirrorPlugins() {
		return [spellingPlugin(this.options)];
	},
});
