// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: lighting find's
// matches in the rows on screen, and the active one apart. Rows come and go as
// the list scrolls, so a painter watches its list for changes to the text find
// reads and paints again on the next frame. It keeps each row's matches until
// that row changes. The rest is MaxHQ's: the highlights belong to the window,
// so each agent view's painter adds and drops only its own ranges.

import type { FindMatch } from "@maxhq/shared/bindings";

import { supportsHighlights } from "../jump";
import { QUOTE_ROW } from "../selection";
import { collectRowRanges, FIND_FIELD } from "./text";

import "./find.css";

/** The highlight that lights every match, which find.css paints. */
export const FIND_HIGHLIGHT = "maxhq-find";
/** The highlight that lights the active match. */
export const FIND_ACTIVE_HIGHLIGHT = "maxhq-find-active";

const FIELD_SELECTOR = `[${FIND_FIELD}]`;

// The window's pair, shared by every painter. A page's CSS may change
// under a test, so each paint checks the pair is still the one set.
let shared: { readonly all: Highlight; readonly active: Highlight } | null = null;

const highlights = () => {
	if (!supportsHighlights()) return null;
	if (shared === null) shared = { all: new Highlight(), active: new Highlight() };
	if (CSS.highlights.get(FIND_HIGHLIGHT) !== shared.all)
		CSS.highlights.set(FIND_HIGHLIGHT, shared.all);
	if (CSS.highlights.get(FIND_ACTIVE_HIGHLIGHT) !== shared.active)
		CSS.highlights.set(FIND_ACTIVE_HIGHLIGHT, shared.active);
	return shared;
};

/** Whether a change touches text find reads. */
const touchesFind = (record: MutationRecord): boolean => {
	const target =
		record.target.nodeType === Node.ELEMENT_NODE
			? (record.target as Element)
			: record.target.parentElement;
	if (target?.closest(FIELD_SELECTOR)) return true;
	const holds = (node: Node) =>
		node.nodeType === Node.ELEMENT_NODE &&
		((node as Element).matches(FIELD_SELECTOR) ||
			(node as Element).querySelector(FIELD_SELECTOR) !== null);
	for (const node of record.addedNodes) if (holds(node)) return true;
	for (const node of record.removedNodes) if (holds(node)) return true;
	return false;
};

/** Lights one agent view's matches. */
export type FindPainter = {
	/** Lights `query`'s matches in the rows on screen, and `match` apart. An empty query clears them. */
	readonly paint: (query: string, match: FindMatch | null) => void;
	/**
	 * The range of `match` in its row, or the row's last one when the row
	 * shows fewer matches than maxd counted. Null when the row shows none.
	 */
	readonly rangeIn: (row: Element, match: FindMatch) => Range | null;
	readonly dispose: () => void;
};

export const createFindPainter = (scroller: HTMLElement): FindPainter => {
	// Each row's matches for `query`, until the row changes.
	const cache = new Map<Element, { readonly item: string | null; readonly ranges: Range[] }>();
	let query = "";
	let match: FindMatch | null = null;
	// The ranges this painter added to the window's pair.
	let painted: Range[] = [];
	let lit: Range | null = null;
	let frame = 0;
	let observer: MutationObserver | null = null;

	const rangesOf = (row: Element): Range[] => {
		const item = row.getAttribute(QUOTE_ROW);
		const kept = cache.get(row);
		// A row the list reused for another item, or whose text went, reads again.
		if (kept?.item === item && kept.ranges.every((range) => row.contains(range.startContainer)))
			return kept.ranges;
		const ranges = collectRowRanges(row, query);
		cache.set(row, { item, ranges });
		return ranges;
	};

	const clear = () => {
		const pair = shared;
		if (pair) {
			for (const range of painted) pair.all.delete(range);
			if (lit) pair.active.delete(lit);
		}
		painted = [];
		lit = null;
	};

	const repaint = () => {
		frame = 0;
		clear();
		const pair = query ? highlights() : null;
		if (!pair) return;
		for (const row of cache.keys()) if (!scroller.contains(row)) cache.delete(row);
		for (const row of scroller.querySelectorAll(`[${QUOTE_ROW}]`)) {
			const ranges = rangesOf(row);
			for (const range of ranges) pair.all.add(range);
			painted.push(...ranges);
			if (match && row.getAttribute(QUOTE_ROW) === match.item) {
				lit = ranges[Math.min(match.occurrence, ranges.length - 1)] ?? null;
				if (lit) pair.active.add(lit);
			}
		}
	};

	const later = () => {
		if (frame === 0) frame = requestAnimationFrame(repaint);
	};

	const watch = () => {
		if (observer !== null || typeof MutationObserver === "undefined") return;
		observer = new MutationObserver((records) => {
			let touched = false;
			for (const record of records) {
				if (!touchesFind(record)) continue;
				touched = true;
				const target =
					record.target.nodeType === Node.ELEMENT_NODE
						? (record.target as Element)
						: record.target.parentElement;
				const row = target?.closest(`[${QUOTE_ROW}]`);
				if (row) cache.delete(row);
			}
			if (touched) later();
		});
		observer.observe(scroller, { childList: true, characterData: true, subtree: true });
	};

	const unwatch = () => {
		observer?.disconnect();
		observer = null;
	};

	return {
		paint: (nextQuery, nextMatch) => {
			if (nextQuery !== query) cache.clear();
			query = nextQuery;
			match = nextMatch;
			if (query) watch();
			else unwatch();
			cancelAnimationFrame(frame);
			repaint();
		},
		rangeIn: (row, at) => {
			if (!query) return null;
			cache.delete(row);
			const ranges = rangesOf(row);
			return ranges[Math.min(at.occurrence, ranges.length - 1)] ?? null;
		},
		dispose: () => {
			cancelAnimationFrame(frame);
			frame = 0;
			unwatch();
			clear();
			cache.clear();
		},
	};
};
