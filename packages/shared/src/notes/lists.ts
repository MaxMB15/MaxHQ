// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What Enter, Tab and Shift-Tab do to a Markdown list in a note's plain text.
// Each takes the text and the selection as offsets into it, and gives back
// the replacements to make and where the selection goes after them, or null
// when the key should do what it always does. Nothing here knows about an
// editor, so the desktop and the phone can share it.
//
// The caller decides whether the caret is in a code block, where a line
// that looks like a list item is only code.

import type { Replacement } from "./diff";

/** A selection as offsets into the text, `anchor` where it started. */
export type OffsetSelection = { readonly anchor: number; readonly head: number };

/** What a key does: replacements in the old text, in order, and the selection in the new one. */
export type ListEdit = {
	readonly replacements: readonly Replacement[];
	readonly selection: OffsetSelection;
};

/** A list item's line, taken apart. Offsets count from the line's start. */
export type ListLine = {
	/** Block quote markers in front of it, such as "> ", kept as written. */
	readonly quote: string;
	/** Where the indent before the marker ends. */
	readonly indentEnd: number;
	/** The indent before the marker, in columns, a tab reaching the next multiple of 4. */
	readonly indent: number;
	/** The marker and the spaces after it, such as "- " or "12. ". */
	readonly marker: string;
	/** The number of an ordered item, as written. */
	readonly number: string | null;
	/** Whether it's a task, with a box after the marker. */
	readonly task: boolean;
	/** Where the item's own text starts, after the marker and any box. */
	readonly contentStart: number;
	/** How far in a child item goes past this one's indent, so it nests. */
	readonly width: number;
};

const QUOTE = /^(?:[ \t]*>[ \t]?)*/;
const ITEM = /^([ \t]*)(?:[-*+]|(\d{1,9})[.)])([ \t]+)(\[[ xX]\](?:[ \t]+|$))?/;
const RULE = /^[ \t]*([-*_])(?:[ \t]*\1){2,}[ \t]*$/;

/** How many lines up a search for a line's parent or sibling looks. */
const SEARCH_LINES = 1_000;

const TAB_STOP = 4;

/** The columns an indent takes. */
const columnsOf = (indent: string): number => {
	let columns = 0;
	for (const char of indent)
		columns = char === "\t" ? columns - (columns % TAB_STOP) + TAB_STOP : columns + 1;
	return columns;
};

/** A line's block quote markers, and its indent after them. */
const leadOf = (line: string) => {
	const quote = QUOTE.exec(line)![0];
	const indent = /^[ \t]*/.exec(line.slice(quote.length))![0];
	return { quote, indentEnd: quote.length + indent.length, indent: columnsOf(indent) };
};

/** A line taken apart as a list item, or null when it isn't one. */
export const listLineOf = (line: string): ListLine | null => {
	const quote = QUOTE.exec(line)![0];
	const rest = line.slice(quote.length);
	if (RULE.test(rest)) return null;
	const match = ITEM.exec(rest);
	if (!match) return null;
	const [whole, indent = "", number, spaces = ""] = match;
	const box = match[4];
	const indentEnd = quote.length + indent.length;
	const markerEnd = quote.length + whole.length - (box?.length ?? 0);
	const marker = line.slice(indentEnd, markerEnd);
	// Five spaces or more after the marker start an indented code block,
	// so the item's text is one space in.
	const width = spaces.length > TAB_STOP ? marker.length - spaces.length + 1 : marker.length;
	return {
		quote,
		indentEnd,
		indent: columnsOf(indent),
		marker,
		number: number ?? null,
		task: box !== undefined,
		contentStart: quote.length + whole.length,
		width,
	};
};

/** The marker the next item starts with: one more for a number, the same for a bullet. */
const nextMarker = (item: ListLine): string => {
	if (item.number === null) return item.marker;
	const next = String(Number(item.number) + 1);
	const padded = item.number.startsWith("0") ? next.padStart(item.number.length, "0") : next;
	return padded + item.marker.slice(item.number.length);
};

/** Where the line holding `offset` starts. */
const lineStartOf = (text: string, offset: number): number =>
	offset === 0 ? 0 : text.lastIndexOf("\n", offset - 1) + 1;

/** Where the line holding `offset` ends, before its line break. */
const lineEndOf = (text: string, offset: number): number => {
	const end = text.indexOf("\n", offset);
	return end === -1 ? text.length : end;
};

/**
 * Where an offset in the old text lands once the replacements are made. Text
 * put in right at it goes after it when `before` is set, and before it
 * otherwise.
 */
const mapOffset = (
	offset: number,
	replacements: readonly Replacement[],
	before = false
): number => {
	let shift = 0;
	for (const { from, to, text } of replacements) {
		if (offset < from || (before && offset === from && from === to)) break;
		if (offset >= to) shift += text.length - (to - from);
		else return from + shift + Math.min(offset - from, text.length);
	}
	return offset + shift;
};

/** The quote markers with their spacing taken out, to tell which quote a line is in. */
const quoteDepth = (quote: string): string => quote.replace(/[ \t]/g, "");

/**
 * The list item a line nests under, looking up from it: the first item in
 * the same quote with less indent than `indent`. The search stops at a line
 * of text that isn't indented, which ends the list.
 */
const itemAbove = (
	text: string,
	lineStart: number,
	quote: string,
	fits: (item: ListLine) => boolean
): ListLine | null => {
	let end = lineStart - 1;
	for (let count = 0; end >= 0 && count < SEARCH_LINES; count++) {
		const start = lineStartOf(text, end);
		const line = text.slice(start, end);
		end = start - 1;
		const item = listLineOf(line);
		if (item) {
			if (quoteDepth(item.quote) !== quoteDepth(quote)) return null;
			if (fits(item)) return item;
			continue;
		}
		const lead = leadOf(line);
		if (line.slice(lead.indentEnd).trim() === "") continue;
		if (quoteDepth(lead.quote) !== quoteDepth(quote) || lead.indent === 0) return null;
	}
	return null;
};

/** How many columns Shift-Tab takes off an item: down to its parent's indent, or its own width. */
const outdentOf = (text: string, lineStart: number, item: ListLine): number => {
	const parent = itemAbove(text, lineStart, item.quote, (each) => each.indent < item.indent);
	return parent ? item.indent - parent.indent : Math.min(item.indent, item.width);
};

/** How many columns Tab adds to an item: its sibling above's width, so it nests there, or its own. */
const indentOf = (text: string, lineStart: number, item: ListLine): number => {
	const above = itemAbove(text, lineStart, item.quote, (each) => each.indent <= item.indent);
	return above?.indent === item.indent ? above.width : item.width;
};

/** The replacement that moves a line's indent to `columns`, in spaces, or null when it's there. */
const reindent = (
	lineStart: number,
	line: string,
	lead: { quote: string; indentEnd: number; indent: number },
	columns: number
): Replacement | null => {
	const from = lineStart + lead.quote.length;
	const to = lineStart + lead.indentEnd;
	const written = line.slice(lead.quote.length, lead.indentEnd);
	if (columns === lead.indent) return null;
	if (written.includes("\t")) return { from, to, text: " ".repeat(columns) };
	if (columns > lead.indent) return { from, to: from, text: " ".repeat(columns - lead.indent) };
	return { from, to: from + lead.indent - columns, text: "" };
};

/**
 * Enter in a list. On an item it starts the next one, with the same quote,
 * indent and marker, a number one more and a task's box empty, and the text
 * after the caret moves into it. On an item with nothing in it the list
 * ends there: a nested item moves out a level as Shift-Tab would, and an
 * item at the left edge loses its marker, keeping any quote. A selection is
 * taken out first. Null when the caret isn't after an item's marker.
 */
export const continueList = (text: string, selection: OffsetSelection): ListEdit | null => {
	const from = Math.min(selection.anchor, selection.head);
	const to = Math.max(selection.anchor, selection.head);
	const lineStart = lineStartOf(text, from);
	const lineEnd = lineEndOf(text, to);
	// The line as it is once the selection is out.
	const line = text.slice(lineStart, from) + text.slice(to, lineEnd);
	const item = listLineOf(line);
	if (!item) return null;
	// A caret on or before the marker, or inside a task's box, breaks the
	// line as usual.
	const caret = from - lineStart;
	const markerEnd = item.indentEnd + item.marker.trimEnd().length;
	if (caret <= markerEnd || (item.task && caret < item.indentEnd + item.marker.length + 3))
		return null;

	if (line.slice(item.contentStart).trim() === "") {
		const kept =
			item.indent > 0
				? item.quote +
					" ".repeat(item.indent - outdentOf(text, lineStart, item)) +
					line.slice(item.indentEnd, item.contentStart)
				: item.quote;
		return {
			replacements: [{ from: lineStart, to: lineEnd, text: kept }],
			selection: { anchor: lineStart + kept.length, head: lineStart + kept.length },
		};
	}

	const prefix = line.slice(0, item.indentEnd) + nextMarker(item) + (item.task ? "[ ] " : "");
	const inserted = "\n" + prefix;
	const next = from + inserted.length;
	return {
		replacements: [{ from, to, text: inserted }],
		selection: { anchor: next, head: next },
	};
};

/**
 * Tab or Shift-Tab on list lines. Tab moves the lines in by the width of the
 * item above at the same level, so the first one nests under it, and
 * Shift-Tab moves them out to the level of the item they nest under. Every
 * line with text in the selection moves by the same amount, so nesting
 * inside it stays, and none goes below the left edge. Null when no line in
 * the selection is a list item.
 */
export const shiftList = (
	text: string,
	selection: OffsetSelection,
	outdent: boolean
): ListEdit | null => {
	const from = Math.min(selection.anchor, selection.head);
	let to = Math.max(selection.anchor, selection.head);
	// A selection ending at a line's start leaves that line alone.
	if (to > from && to > 0 && text[to - 1] === "\n") to -= 1;
	const first = lineStartOf(text, from);
	const last = lineEndOf(text, to);

	const lines: { start: number; line: string }[] = [];
	for (let start = first; start <= last;) {
		const end = lineEndOf(text, start);
		lines.push({ start, line: text.slice(start, end) });
		start = end + 1;
	}
	const leading = lines
		.map(({ start, line }) => ({ start, item: listLineOf(line) }))
		.find((each) => each.item !== null);
	if (!leading?.item) return null;
	const amount = outdent
		? outdentOf(text, leading.start, leading.item)
		: indentOf(text, leading.start, leading.item);

	const replacements: Replacement[] = [];
	for (const { start, line } of lines) {
		const lead = leadOf(line);
		if (line.slice(lead.indentEnd).trim() === "") continue;
		const columns = outdent ? Math.max(0, lead.indent - amount) : lead.indent + amount;
		const replacement = reindent(start, line, lead, columns);
		if (replacement) replacements.push(replacement);
	}
	return {
		replacements,
		// A selection keeps the indent put in at its start, and a caret
		// moves along with its line.
		selection: {
			anchor: mapOffset(
				selection.anchor,
				replacements,
				selection.anchor === from && to > from
			),
			head: mapOffset(selection.head, replacements, selection.head === from && to > from),
		},
	};
};
