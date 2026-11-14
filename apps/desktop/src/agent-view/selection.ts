// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: reading a row's
// text the way it shows, mapping a selection to a passage in it, and finding
// the passage again as a range. The rest is MaxHQ's: any timeline row can be
// quoted, not only replies, so the text skips whatever can't be selected, such
// as a tool row's verb and its buttons.

import { findQuote, type QuoteAnchor, quoteAnchor } from "@maxhq/shared/pills/anchor";

/** The attribute that marks a timeline row and names its item. */
export const QUOTE_ROW = "data-quote-item";

const CONTROL_SELECTOR = "input, textarea, select, [contenteditable]";
/** What a row's text leaves out: controls, hidden parts and pictures. */
export const EXCLUDED_SELECTOR = `${CONTROL_SELECTOR}, [hidden], [aria-hidden=true], script, style, template, noscript, svg`;
/**
 * The elements a row's text reads as blocks of their own. A highlighted
 * code line is a span shown as a block, marked `data-line`, so its lines
 * read apart as the plain code's line breaks do.
 */
export const BLOCK_SELECTOR =
	"address, article, aside, blockquote, dd, div, dl, dt, figcaption, figure, footer, h1, h2, h3, h4, h5, h6, header, hr, li, main, nav, ol, p, pre, section, summary, table, td, th, tr, ul, [data-line]";

type TextChunk = { node: Text; start: number; end: number };

/** Whether a text node's text can't be picked, as with most of the app's chrome. */
const isUnselectable = (text: Text): boolean => {
	const element = text.parentElement;
	const view = element?.ownerDocument.defaultView;
	if (!element || !view) return false;
	const style = view.getComputedStyle(element);
	// WebKit only fills in the prefixed property.
	return (style.userSelect || style.webkitUserSelect) === "none";
};

/** A row's text as it shows, with a line break between blocks. */
export const readRowText = (root: HTMLElement): { text: string; chunks: TextChunk[] } => {
	const parts: string[] = [];
	const chunks: TextChunk[] = [];
	let length = 0;
	let separator = false;

	const append = (node: Text) => {
		if (separator && length > 0) {
			parts.push("\n");
			length += 1;
		}
		separator = false;
		chunks.push({ node, start: length, end: length + node.data.length });
		parts.push(node.data);
		length += node.data.length;
	};

	const visit = (node: Node) => {
		if (node.nodeType === Node.TEXT_NODE) {
			const text = node as Text;
			if (text.length > 0 && !isUnselectable(text)) append(text);
			return;
		}
		if (node.nodeType !== Node.ELEMENT_NODE) return;
		const element = node as Element;
		if (element.matches(EXCLUDED_SELECTOR)) return;
		const block = element.matches(BLOCK_SELECTOR);
		if (block || element.tagName === "BR") separator = true;
		for (const child of element.childNodes) visit(child);
		if (block) separator = true;
	};

	visit(root);
	return { text: parts.join(""), chunks };
};

const excludedAncestor = (node: Node): Element | null => {
	const element = node.nodeType === Node.ELEMENT_NODE ? (node as Element) : node.parentElement;
	return element?.closest(EXCLUDED_SELECTOR) ?? null;
};

const isUsableRange = (root: HTMLElement, range: Range): boolean =>
	!range.collapsed &&
	root.contains(range.startContainer) &&
	root.contains(range.endContainer) &&
	excludedAncestor(range.startContainer) === null &&
	excludedAncestor(range.endContainer) === null;

/** The first or last text node the range picks any of. */
const selectedTextBoundary = (range: Range, node: Node, last: boolean): Text | null => {
	if (!range.intersectsNode(node)) return null;
	if (node.nodeType === Node.TEXT_NODE) {
		const text = node as Text;
		const start = node === range.startContainer ? range.startOffset : 0;
		const end = node === range.endContainer ? range.endOffset : text.length;
		// A triple click picks up the unselectable text after the row, too.
		return start < end && !isUnselectable(text) ? text : null;
	}
	for (
		let child = last ? node.lastChild : node.firstChild;
		child !== null;
		child = last ? child.previousSibling : child.nextSibling
	) {
		const boundary = selectedTextBoundary(range, child, last);
		if (boundary !== null) return boundary;
	}
	return null;
};

/** A selection in one timeline row. */
export type RowSelection = {
	/** The row, marked with {@link QUOTE_ROW}. */
	readonly row: HTMLElement;
	readonly item: string;
	readonly anchor: QuoteAnchor;
	/** What's picked, cut to the row's text. */
	readonly range: Range;
};

/**
 * The passage a selection picks in one row of the timeline in `viewport`.
 * Null when nothing is picked, or when it runs across two rows.
 */
export const captureRowSelection = (
	viewport: HTMLElement,
	selection: Selection | null
): RowSelection | null => {
	if (selection === null || selection.isCollapsed || selection.rangeCount !== 1) return null;
	const range = selection.getRangeAt(0).cloneRange();
	const first = selectedTextBoundary(range, range.commonAncestorContainer, false);
	const last = selectedTextBoundary(range, range.commonAncestorContainer, true);
	if (first === null || last === null) return null;
	const row = first.parentElement?.closest<HTMLElement>(`[${QUOTE_ROW}]`);
	const item = row?.getAttribute(QUOTE_ROW);
	if (!row || !item || !viewport.contains(row)) return null;

	// A paragraph's selection can end at offset 0 of the next block. Check
	// the text actually picked, not that empty end.
	range.setStart(first, first === range.startContainer ? range.startOffset : 0);
	range.setEnd(last, last === range.endContainer ? range.endOffset : last.length);
	if (!isUsableRange(row, range)) return null;

	const stream = readRowText(row);
	let from: number | null = null;
	let to = 0;
	for (const chunk of stream.chunks) {
		if (!range.intersectsNode(chunk.node)) continue;
		const start = range.startContainer === chunk.node ? range.startOffset : 0;
		const end = range.endContainer === chunk.node ? range.endOffset : chunk.end - chunk.start;
		if (start === end) continue;
		from ??= chunk.start + start;
		to = chunk.start + end;
	}
	if (from === null) return null;
	const anchor = quoteAnchor(stream.text, from, to);
	return anchor === null ? null : { row, item, anchor, range };
};

/** The passage a quote picked in a row, or null when the row no longer has it. */
export const resolveQuoteRange = (root: HTMLElement, anchor: QuoteAnchor): Range | null => {
	if (excludedAncestor(root) !== null) return null;
	const stream = readRowText(root);
	const match = findQuote(stream.text, anchor);
	if (match === null) return null;

	const first = stream.chunks.find((chunk) => chunk.end > match.from);
	const last = [...stream.chunks].reverse().find((chunk) => chunk.start < match.to);
	if (first === undefined || last === undefined) return null;

	const range = root.ownerDocument.createRange();
	range.setStart(first.node, Math.max(0, match.from - first.start));
	range.setEnd(last.node, Math.min(last.node.length, match.to - last.start));
	return isUsableRange(root, range) ? range : null;
};
