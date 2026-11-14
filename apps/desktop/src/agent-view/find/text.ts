// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: finding a query's
// matches in the rows on screen, and where the view's top sits among them. The
// rest is MaxHQ's: a row marks the fields maxd counts, and a match never
// crosses a block or a line. A block is each block-level element, and a line
// ends at each `<br>` and each line break in the text. Within a line each run
// of whitespace counts as one space, and text runs on across inline elements
// and the parts find leaves out, such as pills. So a row's matches here are
// the ones maxd counts for its item.

import type { FindMatch } from "@maxhq/shared/bindings";
import { type FindLine, findLines, findOccurrences } from "@maxhq/shared/find/search";

import { BLOCK_SELECTOR, EXCLUDED_SELECTOR, QUOTE_ROW } from "../selection";

/**
 * Marks an element whose text find counts: `plain` for text as it was
 * written, `markdown` for rendered Markdown. Both read the same way.
 */
export const FIND_FIELD = "data-find";
/** Marks a part of a field find leaves out, such as a pill or a copy button. The text on either side runs on. */
export const FIND_SKIP = "data-find-skip";

const FIELD_SELECTOR = `[${FIND_FIELD}]`;
// Marked parts, and the back links and screen reader text of Markdown's footnotes.
const SKIP_SELECTOR = `[${FIND_SKIP}], [data-footnote-backref], .sr-only`;

/** A text node, by where its text starts in a line's. */
type Chunk = { readonly node: Text; readonly start: number };

/** One line's text as find reads it, with the nodes `at` points into. */
type Line = FindLine & { readonly chunks: readonly Chunk[] };

/** A field's lines, in the order it shows them. */
const fieldLines = (field: Element): Line[] => {
	const lines: Line[] = [];
	let raw = "";
	let chunks: Chunk[] = [];

	const flush = () => {
		for (const line of findLines(raw)) lines.push({ ...line, chunks });
		raw = "";
		chunks = [];
	};

	const visit = (node: Node) => {
		if (node.nodeType === Node.TEXT_NODE) {
			const text = node as Text;
			if (text.length === 0) return;
			chunks.push({ node: text, start: raw.length });
			raw += text.data;
			return;
		}
		if (node.nodeType !== Node.ELEMENT_NODE) return;
		const element = node as Element;
		if (element.matches(EXCLUDED_SELECTOR) || element.matches(SKIP_SELECTOR)) return;
		if (element.tagName === "BR") return flush();
		const block = element.matches(BLOCK_SELECTOR);
		if (block) flush();
		for (const child of element.childNodes) visit(child);
		if (block) flush();
	};

	visit(field);
	flush();
	return lines;
};

/** Where the character at `index` of the nodes' text sits. */
const pointAt = (chunks: readonly Chunk[], index: number): { node: Text; offset: number } => {
	let low = 0;
	let high = chunks.length - 1;
	while (low < high) {
		const middle = (low + high + 1) >> 1;
		if (chunks[middle]!.start <= index) low = middle;
		else high = middle - 1;
	}
	const chunk = chunks[low]!;
	return { node: chunk.node, offset: index - chunk.start };
};

/**
 * The matches of `query` in a row, in the order maxd counts them: field
 * by field, line by line. `query` is find's query, already trimmed.
 */
export const collectRowRanges = (row: Element, query: string): Range[] => {
	const ranges: Range[] = [];
	if (query.length === 0) return ranges;
	for (const field of row.querySelectorAll(FIELD_SELECTOR)) {
		// A field inside another counts as part of it.
		if (field.parentElement?.closest(FIELD_SELECTOR)) continue;
		for (const line of fieldLines(field)) {
			for (const offset of findOccurrences(line.text, query)) {
				// A trimmed query starts and ends on a character the nodes have.
				const start = pointAt(line.chunks, line.at[offset]!);
				const end = pointAt(line.chunks, line.at[offset + query.length - 1]!);
				const range = row.ownerDocument.createRange();
				range.setStart(start.node, start.offset);
				range.setEnd(end.node, end.offset + 1);
				ranges.push(range);
			}
		}
	}
	return ranges;
};

/**
 * The first match at or below the top of the view, as find's start: the
 * first row in view, and the first of its matches that ends below the
 * view's top, or the row's end when none does.
 */
export const readFindPosition = (scroller: HTMLElement, query: string): FindMatch | undefined => {
	const view = scroller.getBoundingClientRect();
	for (const row of scroller.querySelectorAll(`[${QUOTE_ROW}]`)) {
		const rect = row.getBoundingClientRect();
		if (rect.bottom <= view.top || rect.top >= view.bottom) continue;
		const item = row.getAttribute(QUOTE_ROW);
		if (!item) continue;
		const ranges = collectRowRanges(row, query);
		const next = ranges.findIndex((range) => range.getBoundingClientRect().bottom > view.top);
		return { item, occurrence: next < 0 ? ranges.length : next };
	}
	return undefined;
};
