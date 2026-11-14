// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Markdown in the text box shows as it will read: bold is bold and code is
// code. The text stays the plain Markdown the agent gets, and the look is
// drawn over it, so drafts, offsets, pills and undo never see it. The
// grammar is the one a sent message renders with. The markers around a span
// hide once the caret or the selection is off it, and come back to edit it.
// They show only while the editor has the focus, since a selection left at
// the start of a note it just opened isn't anyone editing there. They stay
// put while a press is down and for a moment after, since text that moved
// under a double click would select another word. Fences and list markers
// always show, so a block's lines don't jump. A quote's `>` marks hide while
// the caret is off their line, and each depth draws a bar of its own.
//
// A Mermaid block, a formula block, a table and a callout draw as they will
// read once the selection is off their lines, and a click or an arrow key
// brings their source back, the way a formula in a sentence does. A footnote
// call draws as its number.
//
// The text is read one top-level block at a time, the blocks split at blank
// lines outside fences. A change reads again only the blocks it touched, and
// every other block keeps its look and its decorations, moved along with the
// text, so a key costs the same in a long note as in a short message. A link
// or footnote definition counts across the whole text, so each block reads
// with every label the text defines, and a change to those labels reads all
// of it again.
//
// A note turns on more of it: markers show while the caret is on their line,
// a fence's while the caret is in its block, a task's box ticks on a click,
// Command-click opens a link, and a picture on a line of its own draws. A
// note also draws the raw HTML a reply would, such as `<mark>` in a sentence
// and a `<details>` block. The composer leaves HTML as text, since a message
// you send shows its HTML as text too.
//
// Given a host for them, a closed `html visual` fence draws as its visual's
// card, which the host draws. Its lines hide even while the caret is near,
// and it opens only when asked or when the caret lands inside it, then
// closes again once the selection leaves it. The arrows, Backspace and
// Delete select it whole rather than stopping in its hidden lines, and Enter
// on it opens its source.

import { Extension } from "@tiptap/core";
import type { Node as ProseNode } from "@tiptap/pm/model";
import {
	type EditorState,
	Plugin,
	PluginKey,
	TextSelection,
	type Transaction,
} from "@tiptap/pm/state";
import { Decoration, DecorationSet, type EditorView } from "@tiptap/pm/view";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import remarkParse from "remark-parse";
import { unified } from "unified";

import { guardDollars, restoreDollars } from "@maxhq/shared/agent-view/markdown";
import { resolveMediaSource } from "@maxhq/shared/agent-view/media-source";
import { OBJECT_MARK } from "@maxhq/shared/pills/draft";
import { fromVisualFence, type Visual, visualFence } from "@maxhq/shared/visuals/fence";
import { visualKey } from "@maxhq/shared/visuals/visual";
import { alertTone } from "@/agent-view/alert";
import { alertLine, drawsPairedTag } from "@/agent-view/markdown-plugins";

import { blockPreview, type BlockKind, formulaPreview, removeBlockPreview } from "./previews";
import { t as translate } from "@maxhq/shared/i18n";

/**
 * The longest text that gets the look in the composer, past which the box
 * stays plain. Reading only the changed block keeps typing quick at any
 * length, but a paste still reads everything it brings, which takes about a
 * frame at this length.
 */
export const MAX_MARKDOWN_LENGTH = 5_000;

/**
 * The longest block that gets the look. Every key in a block reads all of
 * it again, so past this a block, such as a long fence, stays plain.
 */
export const MAX_BLOCK_LENGTH = 50_000;

/** How long after a press lets go the markers stay put, about a double click's gap. */
export const MARKERS_HELD_MS = 250;

/** What the look draws, which a note turns more of on. */
export type MarkdownLookOptions = {
	/** The longest text that gets the look, past which it stays plain. */
	readonly maxLength: number;
	/** Markers show while the selection is on their lines, and a fence's while it's in the block. */
	readonly byLine: boolean;
	/** A task's `[ ]` draws as a box, and a click ticks it. */
	readonly tasks: boolean;
	/** Command-click opens a link. */
	readonly links: boolean;
	/** A picture on a line of its own draws, when it needs no agent's device. */
	readonly images: boolean;
	/** Raw HTML that a reply would draw draws, such as `<kbd>` or a `<details>` block. */
	readonly html: boolean;
	/** Where an `html visual` fence draws as its card, which only a note has. */
	readonly visuals?: VisualHost;
};

/** What a drawn visual's card can ask of the text around it. */
export type VisualControls = {
	/** Selects the whole block, so Delete and copy act on all of it. */
	readonly select: () => void;
	/** Shows the block's fence as text to edit, with the caret in it. */
	readonly showSource: () => void;
};

/** Draws a visual's card into a node of the text, and lets go of it again. */
export type VisualHost = {
	readonly draw: (dom: HTMLElement, visual: Visual, controls: VisualControls) => void;
	readonly remove: (dom: HTMLElement) => void;
};

/** The composer's look. */
export const COMPOSER_LOOK: MarkdownLookOptions = {
	maxLength: MAX_MARKDOWN_LENGTH,
	byLine: false,
	tasks: false,
	links: false,
	images: false,
	html: false,
};

type Range = { readonly from: number; readonly to: number };

/** Text drawn with a class. */
type Run = Range & { readonly className: string };

/** A marker, which shows while the selection touches `within`, or always when that's null. */
type Marker = Range & {
	readonly within: Range | null;
	/** A line that folds away while the marker hides, since nothing else is on it. */
	readonly fold?: number;
	/** A `<br>`, which breaks the line while it hides. */
	readonly lineBreak?: true;
};

/** Lines by their index, from the first through the last. */
type Fence = { readonly first: number; readonly last: number };

/** The code inside a top-level fence that names a language, from where its first line starts. */
export type FenceCode = {
	readonly lang: string;
	readonly from: number;
	readonly text: string;
};

/** A formula in a sentence, drawn while the selection is off it, or off `within` when set. */
type Formula = Range & { readonly source: string; readonly within?: Range };

/** A block drawn as it will read while the selection is off `within`. */
type Preview = {
	readonly kind: BlockKind | "visual";
	readonly source: string;
	/** Its lines by their index, from the first through the last. */
	readonly first: number;
	readonly last: number;
	readonly within: Range;
	/** Where the caret goes to edit it. */
	readonly caret: number;
	/** Drawn as a reply draws it, not as a message you send. */
	readonly reply?: true;
	/**
	 * A visual's page, drawn by `host` wherever the selection is, until its
	 * source is opened, and the key its drawing is kept by.
	 */
	readonly visual?: { readonly page: Visual; readonly host: VisualHost; readonly key: string };
};

/** A task's `[ ]` or `[x]`, drawn as a box while the selection is off `within`. */
type Task = Range & { readonly checked: boolean; readonly within: Range };

/** A link, which Command-click opens. A reference link's address is its definition's. */
type Link = Range & ({ readonly url: string } | { readonly ref: string });

/** A footnote call such as `[^1]`, drawn as its number while the selection is off `within`. */
type Footnote = Range & {
	readonly within: Range;
	/** Its definition's identifier. */
	readonly id: string;
	/** Whether it's in a footnote's own text, which a reply numbers after the rest. */
	readonly inDefinition: boolean;
};

/** The link and footnote definitions of a text, by their identifiers, in the text's order. */
export type Definitions = {
	/** Each link definition's label as written, and its address. */
	readonly links: ReadonlyMap<string, { readonly label: string; readonly url: string }>;
	/** Each footnote definition's label as written. */
	readonly notes: ReadonlyMap<string, string>;
};

export const NO_DEFINITIONS: Definitions = { links: new Map(), notes: new Map() };

/** How Markdown text looks. Every range is a document position, not an offset in the text. */
export type Look = {
	readonly runs: readonly Run[];
	readonly markers: readonly Marker[];
	/** Each line's classes, by the line's index. */
	readonly lines: ReadonlyMap<number, string>;
	/** The lines of each fenced code block where Enter adds a line. */
	readonly fences: readonly Fence[];
	/** The code of each fence that names a language, for code-colours.ts. */
	readonly code: readonly FenceCode[];
	readonly formulas: readonly Formula[];
	readonly previews: readonly Preview[];
	readonly tasks: readonly Task[];
	readonly links: readonly Link[];
	readonly footnotes: readonly Footnote[];
	/** The definitions the text holds itself. */
	readonly defines: Definitions;
};

const NO_LOOK: Look = {
	runs: [],
	markers: [],
	lines: new Map(),
	fences: [],
	code: [],
	formulas: [],
	previews: [],
	tasks: [],
	links: [],
	footnotes: [],
	defines: NO_DEFINITIONS,
};

// Just enough of a syntax tree node to read where each thing is.
type Point = { readonly line: number; readonly offset?: number | undefined };
type TreeNode = {
	readonly type: string;
	readonly depth?: number;
	readonly lang?: string | null;
	readonly meta?: string | null;
	readonly value?: string;
	readonly url?: string;
	readonly identifier?: string;
	readonly label?: string | null;
	readonly checked?: boolean | null;
	readonly position?: { readonly start: Point; readonly end: Point };
	readonly children?: readonly TreeNode[];
};

const PARSER = unified().use(remarkParse).use(remarkGfm).use(remarkMath);

const INLINE_LOOKS: Readonly<Record<string, string>> = {
	strong: "md-strong",
	emphasis: "md-em",
	delete: "md-strike",
	link: "md-link",
};

const OPENING_FENCE = /^(`{3,}|~{3,})/;
const OPENING_MATH = /^(\${2,})/;
const CLOSING_FENCE = /^[\s>]*(`{3,}|~{3,}|\${2,})\s*$/;
const SETEXT_LINE = /[=-]+[ \t]*$/;
const TASK_BOX = /^\s*(?:[-*+]|\d+[.)])\s+(\[[ xX]\])/;
// What a backslash escapes.
const PUNCTUATION = /[!-/:-@[-`{-~]/;
// The deepest quote that draws a bar of its own.
const DEEPEST_QUOTE = 6;

// Each line break in the text is two positions in the document, between
// paragraphs, and the first paragraph's text starts at position 1.
const positionOf = (point: Point): number => (point.offset ?? 0) + point.line;

/** Where a line's `depth`th quote mark is, with the space after it, or null on a lazy line. */
const quoteMark = (line: string, depth: number): Range | null => {
	const mark = / {0,3}> ?/y;
	for (let each = 1; each <= depth; each++) {
		const at = mark.lastIndex;
		if (!mark.exec(line)) return null;
		if (each === depth) return { from: line.indexOf(">", at), to: mark.lastIndex };
	}
	return null;
};

/**
 * The top-level nodes of a block. A reference or a footnote call only reads
 * as one when its definition is in the text, and a block's text is only
 * part of it, so the text's other definitions go after the block, past a
 * blank line, and are dropped again. A block that runs on into them, such as
 * an open fence, reads on its own.
 */
const parseBlock = (text: string, defined: Definitions): readonly TreeNode[] => {
	if (text.includes("[") && (defined.links.size > 0 || defined.notes.size > 0)) {
		let after = "\n";
		for (const { label } of defined.links.values()) after += `\n[${label}]: x`;
		for (const label of defined.notes.values()) after += `\n[^${label}]: x`;
		const lines = text.split("\n").length;
		const root = PARSER.parse(guardDollars(text + after)) as TreeNode;
		const own = (root.children ?? []).filter(
			(node) => (node.position?.start.line ?? 0) <= lines
		);
		if ((own.at(-1)?.position?.end.line ?? 0) <= lines) return own;
	}
	return (PARSER.parse(guardDollars(text)) as TreeNode).children ?? [];
};

// A whole tag in raw HTML, and whether it closes.
const TAG = /^<(\/?)([a-zA-Z][a-zA-Z0-9-]*)(?:\s[^>]*)?>$/;
const TAGS = /<(\/?)([a-zA-Z][a-zA-Z0-9-]*)(?:"[^"]*"|'[^']*'|[^'">])*?(\/?)>/g;
const COMMENTS = /<!--[\s\S]*?-->/g;
const LINE_BREAK_TAG = /^<br\s*\/?>$/i;
const OPENING_TAG = /^ {0,3}<([a-zA-Z][a-zA-Z0-9-]*)(?=[\s/>]|$)/;
// What could carry a picture, which a drawing in the text box leaves out.
const EMBEDS = /<(?:img|video|picture)(?![a-zA-Z0-9-])/i;

/** The tag some HTML opens with, when a reply draws it and it takes a pair. */
const openingTag = (html: string): string | null => {
	const name = OPENING_TAG.exec(html)?.[1]?.toLowerCase();
	return name && drawsPairedTag(name) ? name : null;
};

/** How many more `name` tags the HTML opens than it closes. */
const depthOf = (html: string, name: string): number => {
	let depth = 0;
	for (const [, closing, tag, selfClosing] of html.replace(COMMENTS, "").matchAll(TAGS))
		if (tag?.toLowerCase() === name && !selfClosing) depth += closing ? -1 : 1;
	return depth;
};

// The parents whose raw HTML sits in a sentence, where its tags pair up.
const PHRASING = new Set([
	"paragraph",
	"heading",
	"strong",
	"emphasis",
	"delete",
	"link",
	"linkReference",
	"tableCell",
]);

/** Whether a selection from `from` to `to` touches a range. */
const touches = (range: Range, from: number, to: number): boolean =>
	from <= range.to && to >= range.from;

/**
 * How `text` looks, which may be nothing at all. `defined` holds the
 * definitions elsewhere in the text it's part of.
 */
export const markdownLook = (
	text: string,
	options: MarkdownLookOptions = COMPOSER_LOOK,
	defined: Definitions = NO_DEFINITIONS
): Look => {
	if (text.length > options.maxLength) return NO_LOOK;
	const runs: Run[] = [];
	const markers: Marker[] = [];
	const lines = new Map<number, string>();
	const fences: Fence[] = [];
	const code: FenceCode[] = [];
	const formulas: Formula[] = [];
	const previews: Preview[] = [];
	const tasks: Task[] = [];
	const links: Link[] = [];
	const footnotes: Footnote[] = [];
	const ownLinks = new Map<string, { label: string; url: string }>();
	const ownNotes = new Map<string, string>();
	// Each quoted line's depth, the deepest quote it's in.
	const quoteDepths = new Map<number, number>();
	let quoteDepth = 0;
	let inDefinition = false;
	const sourceLines = text.split("\n");
	// Where each line's text starts in the document.
	const lineStarts: number[] = [];
	sourceLines.reduce((start, line) => {
		lineStarts.push(start);
		return start + line.length + 2;
	}, 1);

	/** The line a document position is on. */
	const lineOf = (pos: number): number => {
		let low = 0;
		let high = lineStarts.length - 1;
		while (low < high) {
			const middle = (low + high + 1) >> 1;
			if ((lineStarts[middle] ?? 0) <= pos) low = middle;
			else high = middle - 1;
		}
		return low;
	};
	/** The lines from the first through the last, as one range. */
	const linesRange = (first: number, last: number): Range => ({
		from: lineStarts[first] ?? 0,
		to: (lineStarts[last] ?? 0) + (sourceLines[last]?.length ?? 0),
	});
	/** Where markers show: the span itself, or in a note the lines it's on. */
	const reveal = (range: Range): Range =>
		options.byLine ? linesRange(lineOf(range.from), lineOf(range.to)) : range;

	const addLine = (line: number, className: string) => {
		const had = lines.get(line);
		lines.set(line, had ? `${had} ${className}` : className);
	};
	const addMarker = (from: number, to: number, within: Range | null, fold?: number) => {
		if (from < to)
			markers.push(fold === undefined ? { from, to, within } : { from, to, within, fold });
	};

	// A span with markers around its children.
	const wrapped = (node: TreeNode, className: string | null) => {
		const { position, children } = node;
		const first = children?.[0]?.position;
		const last = children?.at(-1)?.position;
		if (!position || !first || !last) return;
		const span = { from: positionOf(position.start), to: positionOf(position.end) };
		const within = reveal(span);
		const from = positionOf(first.start);
		const to = positionOf(last.end);
		if (className && from < to) runs.push({ from, to, className });
		addMarker(span.from, from, within);
		addMarker(to, span.to, within);
	};

	const formula = ({ position, value }: TreeNode) => {
		// Hiding a line break would join its two lines.
		if (!position || position.start.line !== position.end.line) return;
		const source = restoreDollars(value ?? "");
		if (source.trim() === "" || source.includes(OBJECT_MARK)) return;
		const range = { from: positionOf(position.start), to: positionOf(position.end) };
		formulas.push(
			options.byLine ? { ...range, source, within: reveal(range) } : { ...range, source }
		);
	};

	const inlineCode = ({ position }: TreeNode) => {
		if (!position) return;
		const span = { from: positionOf(position.start), to: positionOf(position.end) };
		const within = reveal(span);
		let ticks = 0;
		while (text[(position.start.offset ?? 0) + ticks] === "`") ticks++;
		const from = span.from + ticks;
		const to = span.to - ticks;
		if (from < to) runs.push({ from, to, className: "md-code" });
		addMarker(span.from, from, within);
		addMarker(to, span.to, within);
	};

	const codeBlock = (node: TreeNode) => {
		const { position } = node;
		if (!position) return;
		const first = position.start.line - 1;
		const last = position.end.line - 1;
		for (let line = first; line <= last; line++) addLine(line, "md-code-line");
		addLine(first, "md-code-first");
		addLine(last, "md-code-last");
		const start = position.start.offset ?? 0;
		const opening = node.type === "math" ? OPENING_MATH : OPENING_FENCE;
		const fence = opening.exec(text.slice(start, start + 256))?.[1];
		if (!fence) return;
		addLine(first, "md-fence");
		const closer = last > first ? CLOSING_FENCE.exec(sourceLines[last] ?? "")?.[1] : undefined;
		const closed = closer?.[0] === fence[0] && (closer?.length ?? 0) >= fence.length;
		if (closed) addLine(last, "md-fence");
		fences.push({ first, last: closed ? last - 1 : last });
		// Its code, when its lines hold it as written, with no indent or marks taken off.
		const end = closed ? last - 1 : last;
		if (node.type === "code" && node.lang && topLevel.has(node) && end > first) {
			const inside = sourceLines.slice(first + 1, end + 1).join("\n");
			if (inside === restoreDollars(node.value ?? ""))
				code.push({ lang: node.lang, from: lineStarts[first + 1] ?? 0, text: inside });
		}
		// In a note a fence's own lines fold away while the caret is out of
		// its block. One in a list or a quote shares its line with their marks.
		if (options.byLine && topLevel.has(node)) {
			const block = linesRange(first, last);
			const foldLine = (line: number) => {
				const { from, to } = linesRange(line, line);
				addMarker(from, to, block, line);
			};
			foldLine(first);
			if (closed) foldLine(last);
		}
		const kind = node.type === "math" ? "math" : node.lang === "mermaid" ? "mermaid" : null;
		const source = node.value ?? "";
		if (kind && closed && source.trim() !== "")
			addPreview(node, kind, restoreDollars(source), first, last, first + 1);
		const named = node.type === "code" && closed ? visualFence(node.lang, node.meta) : null;
		if (named && options.visuals) {
			const html = sourceLines.slice(first + 1, last).join("\n");
			if (html.trim() !== "")
				addPreview(node, "visual", html, first, last, first + 1, {
					page: { html, title: named.title },
					host: options.visuals,
					key: visualKey(sourceLines.slice(first, last + 1).join("\n")),
				});
		}
	};

	const listItem = (node: TreeNode) => {
		const { position } = node;
		if (!position) return;
		const first = node.children?.[0]?.position;
		const from = positionOf(position.start);
		const end = first ? positionOf(first.start) : positionOf(position.end);
		const line = position.start.line - 1;
		const box =
			options.tasks && typeof node.checked === "boolean"
				? TASK_BOX.exec(sourceLines[line] ?? "")
				: null;
		if (!box?.[1]) {
			addMarker(from, end, null);
			return;
		}
		const lineStart = lineStarts[line] ?? 0;
		const at = lineStart + box[0].length - box[1].length;
		const within = linesRange(line, line);
		// The box stands in for the list's dash, so the dash hides with it.
		addMarker(from, at, within);
		tasks.push({ from: at, to: at + 3, checked: node.checked === true, within });
		if (!node.checked) return;
		addLine(line, "md-task-done");
		const words = (sourceLines[line] ?? "").slice(box[0].length).search(/\S/);
		if (words >= 0)
			runs.push({ from: at + 3 + words, to: within.to, className: "md-task-done-text" });
	};

	// A picture on a line of its own, which in a note draws as it reads. One
	// on an agent's device stays as text, since a note has no agent to ask.
	const picture = (node: TreeNode) => {
		const only = node.children?.length === 1 ? node.children[0] : undefined;
		const { position } = node;
		if (only?.type !== "image" || !position || position.start.line !== position.end.line)
			return;
		const media = resolveMediaSource(only.url, { imageEmbed: true });
		if (!media || media.access === "device") return;
		const line = position.start.line - 1;
		addPreview(node, "image", sourceLines[line] ?? "", line, line, line);
	};

	// Raw HTML in a sentence. A tag a reply draws and its closing tag, side
	// by side in one parent, hide around what they hold, which takes the
	// tag's look. Any other tag stays as text, as a reply shows it. A `<br>`
	// breaks the line while it hides.
	const pairTags = (children: readonly TreeNode[]) => {
		const open: { readonly name: string; readonly tag: Range }[] = [];
		for (const child of children) {
			const { position, value = "" } = child;
			if (child.type !== "html" || !position || value.includes("\n")) continue;
			const tag = { from: positionOf(position.start), to: positionOf(position.end) };
			if (LINE_BREAK_TAG.test(value)) {
				markers.push({ ...tag, within: reveal(tag), lineBreak: true });
				continue;
			}
			const match = TAG.exec(value);
			const name = match?.[2]?.toLowerCase();
			if (!match || !name || value.endsWith("/>") || !drawsPairedTag(name)) continue;
			if (!match[1]) {
				open.push({ name, tag });
				continue;
			}
			const at = open.findLastIndex((each) => each.name === name);
			const opener = open[at];
			if (!opener) continue;
			open.splice(at);
			const within = reveal({ from: opener.tag.from, to: tag.to });
			if (opener.tag.to < tag.from)
				runs.push({ from: opener.tag.to, to: tag.from, className: `md-tag-${name}` });
			addMarker(opener.tag.from, opener.tag.to, within);
			addMarker(tag.from, tag.to, within);
		}
	};

	// Raw HTML on lines of its own, such as `<details>`, draws as a reply
	// shows it, from its opening tag through its closing one. Markdown after
	// a blank line inside it reads as blocks of its own.
	const htmlBlocks = (nodes: readonly TreeNode[]) => {
		for (let index = 0; index < nodes.length; index++) {
			const opener = nodes[index]!;
			const name = opener.type === "html" ? openingTag(opener.value ?? "") : null;
			if (!name || !opener.position) continue;
			let depth = depthOf(opener.value ?? "", name);
			let close = index;
			while (depth > 0 && ++close < nodes.length) {
				const each = nodes[close]!;
				if (each.type === "html") depth += depthOf(each.value ?? "", name);
			}
			const end = nodes[close]?.position?.end;
			if (depth > 0 || !end) continue;
			const { start } = opener.position;
			const source = text.slice(start.offset, end.offset);
			addPreview(opener, "html", source, start.line - 1, end.line - 1, start.line - 1);
			index = close;
		}
	};

	// A heading with a line of `=` or `-` under it. The line is its marker,
	// which in a note folds away as a fence's does while the caret is off it.
	const underlined = (node: TreeNode, level: number) => {
		const { position } = node;
		if (!position) return;
		const first = position.start.line - 1;
		const last = position.end.line - 1;
		for (let line = first; line < last; line++) addLine(line, `md-h${level}`);
		const start = lineStarts[last];
		const underline = SETEXT_LINE.exec(sourceLines[last] ?? "");
		if (start === undefined || !underline) return;
		const to = start + (sourceLines[last]?.length ?? 0);
		if (!options.byLine) addMarker(start + underline.index, to, null);
		else if (topLevel.has(node)) addMarker(start, to, linesRange(first, last), last);
		else addMarker(start + underline.index, to, linesRange(first, last));
	};

	// A quote at `depth`. Its own `>` on each of its lines hides while the
	// caret is off the line. One that starts with `[!NOTE]` is a callout,
	// which on lines of its own draws whole, and otherwise takes its colour
	// with its `[!NOTE]` dimmed.
	const quote = (node: TreeNode, depth: number) => {
		const { position } = node;
		if (!position) return;
		const first = position.start.line - 1;
		const last = position.end.line - 1;
		for (let line = first; line <= last; line++) {
			quoteDepths.set(line, Math.max(quoteDepths.get(line) ?? 0, depth));
			const mark = quoteMark(sourceLines[line] ?? "", depth);
			const start = lineStarts[line];
			if (mark && start !== undefined)
				addMarker(start + mark.from, start + mark.to, linesRange(line, line));
		}
		const paragraph = node.children?.[0];
		const opener = paragraph?.type === "paragraph" ? paragraph.children?.[0] : undefined;
		const alert = opener?.type === "text" ? alertLine(opener.value ?? "") : null;
		if (!alert || !opener?.position) return;
		for (let line = first; line <= last; line++)
			addLine(line, `md-callout md-tone-${alertTone(alert.kind)}`);
		const from = positionOf(opener.position.start);
		addMarker(from, from + alert.mark, null);
		const source = text.slice(position.start.offset, position.end.offset);
		addPreview(node, "callout", source, first, last, first);
	};

	// The backslash of each escape in some text, which hides as a marker.
	const escapes = ({ position }: TreeNode) => {
		if (!position) return;
		const start = position.start.offset ?? 0;
		const source = text.slice(start, position.end.offset);
		for (let at = source.indexOf("\\"); at !== -1; at = source.indexOf("\\", at + 2)) {
			if (!PUNCTUATION.test(source[at + 1] ?? "")) continue;
			// Text can run over lines, and each line break is two positions.
			const breaks = source.slice(0, at).split("\n").length - 1;
			const from = positionOf(position.start) + at + breaks;
			addMarker(from, from + 1, reveal({ from, to: from + 2 }));
		}
	};

	const walk = (node: TreeNode) => {
		const { position } = node;
		switch (node.type) {
			case "text":
				return escapes(node);
			// A backslash that breaks the line. Each line is a paragraph of
			// its own here, so the line already ends there.
			case "break":
				if (position && text[position.start.offset ?? 0] === "\\") {
					const from = positionOf(position.start);
					addMarker(from, from + 1, reveal({ from, to: from + 1 }));
				}
				return;
			case "linkReference":
				wrapped(node, INLINE_LOOKS.link ?? null);
				if (options.links && position && node.identifier)
					links.push({
						from: positionOf(position.start),
						to: positionOf(position.end),
						ref: node.identifier,
					});
				break;
			// A link's definition shows, dimmed, as a footnote's label does.
			case "definition":
				if (!position) return;
				if (node.identifier && node.label && !ownLinks.has(node.identifier))
					ownLinks.set(node.identifier, { label: node.label, url: node.url ?? "" });
				addMarker(positionOf(position.start), positionOf(position.end), null);
				return;
			case "footnoteReference": {
				if (!position || !node.identifier) return;
				const call = { from: positionOf(position.start), to: positionOf(position.end) };
				footnotes.push({
					...call,
					within: reveal(call),
					id: node.identifier,
					inDefinition,
				});
				return;
			}
			case "footnoteDefinition": {
				if (!position) return;
				if (node.identifier && node.label && !ownNotes.has(node.identifier))
					ownNotes.set(node.identifier, node.label);
				const start = position.start.offset ?? 0;
				const end = text.indexOf("]:", start);
				if (end !== -1)
					addMarker(positionOf(position.start), end + 2 + position.start.line, null);
				inDefinition = true;
				node.children?.forEach(walk);
				inDefinition = false;
				return;
			}
			case "strong":
			case "emphasis":
			case "delete":
				wrapped(node, INLINE_LOOKS[node.type] ?? null);
				break;
			case "link":
				wrapped(node, INLINE_LOOKS.link ?? null);
				if (options.links && position && node.url)
					links.push({
						from: positionOf(position.start),
						to: positionOf(position.end),
						url: node.url,
					});
				break;
			case "inlineCode":
				return inlineCode(node);
			case "code":
			case "math":
				return codeBlock(node);
			case "inlineMath":
				return formula(node);
			case "paragraph":
				if (options.images && topLevel.has(node)) picture(node);
				break;
			// A table's pipes only line up with every marker showing, so it
			// draws whole.
			case "table":
				if (position)
					addPreview(
						node,
						"table",
						text.slice(position.start.offset, position.end.offset),
						position.start.line - 1,
						position.end.line - 1,
						position.start.line - 1
					);
				return;
			// A rule on a line of its own draws as a line once the selection is
			// off it. One in a list or a quote shares its line with their marks.
			case "thematicBreak": {
				if (!position) return;
				const rule = { from: positionOf(position.start), to: positionOf(position.end) };
				if (!topLevel.has(node)) return addMarker(rule.from, rule.to, null);
				const line = position.start.line - 1;
				addLine(line, "md-rule");
				addMarker(rule.from, rule.to, reveal(rule), line);
				return;
			}
			case "heading": {
				if (!position) break;
				const level = Math.min(node.depth ?? 1, 4);
				// Only an underlined heading takes more than one line.
				if (position.end.line > position.start.line) underlined(node, level);
				else {
					addLine(position.start.line - 1, `md-h${level}`);
					if (node.children?.length) wrapped(node, null);
					else addMarker(positionOf(position.start), positionOf(position.end), null);
				}
				break;
			}
			// A comment shows, dimmed, wherever it is.
			case "html":
				if (options.html && position && node.value?.startsWith("<!--"))
					runs.push({
						from: positionOf(position.start),
						to: positionOf(position.end),
						className: "md-comment",
					});
				return;
			case "blockquote":
				quote(node, ++quoteDepth);
				node.children?.forEach(walk);
				quoteDepth--;
				return;
			case "listItem":
				listItem(node);
				break;
		}
		if (options.html && node.children && PHRASING.has(node.type)) pairTags(node.children);
		node.children?.forEach(walk);
	};

	const nodes = parseBlock(text, defined);
	const topLevel = new Set(nodes);

	// Only a block on lines of its own hides them, since one in a list or a
	// quote shares them with the list's or the quote's marks. A pill has no
	// place in a drawing, and a picture in a table would need the feed's own
	// services.
	const addPreview = (
		node: TreeNode,
		kind: BlockKind | "visual",
		source: string,
		first: number,
		last: number,
		caretLine: number,
		visual?: Preview["visual"]
	) => {
		if (!topLevel.has(node) || source.includes(OBJECT_MARK)) return;
		// A page's script may hold `![` and still be a page.
		if (kind !== "image" && kind !== "visual" && source.includes("![")) return;
		if (kind === "html" && EMBEDS.test(source)) return;
		const from = lineStarts[first];
		const end = lineStarts[last];
		const caret = lineStarts[caretLine];
		if (from === undefined || end === undefined || caret === undefined) return;
		const to = end + (sourceLines[last]?.length ?? 0);
		const preview = { kind, source, first, last, within: { from, to }, caret };
		if (visual) previews.push({ ...preview, visual });
		else
			previews.push(
				kind === "callout" && options.html ? { ...preview, reply: true } : preview
			);
	};

	nodes.forEach(walk);
	if (options.html) htmlBlocks(nodes);
	for (const [line, depth] of quoteDepths)
		addLine(
			line,
			depth > 1 ? `md-quote md-quote-${Math.min(depth, DEEPEST_QUOTE)}` : "md-quote"
		);
	const defines =
		ownLinks.size > 0 || ownNotes.size > 0
			? { links: ownLinks, notes: ownNotes }
			: NO_DEFINITIONS;
	return {
		runs,
		markers,
		lines,
		fences,
		code,
		formulas,
		previews,
		tasks,
		links,
		footnotes,
		defines,
	};
};

/** Whether a marker shows while the selection is from `from` to `to`. */
const showing = (marker: Marker, from: number, to: number): boolean =>
	marker.within === null || touches(marker.within, from, to);

// One top-level block of the text: its paragraphs from `from` to `to` in the
// document, and its look with every position and line counted from the
// block's own start, as `markdownLook` gives it for the block's text alone.
type Read = {
	readonly from: number;
	readonly to: number;
	readonly lines: number;
	readonly look: Look;
};

// A block's decorations, counted from its own start, so a block the text
// moves keeps them as they are. The ones on its lines and between them go in
// `outer`, the ones inside a line in `inner`, a set over the block alone.
type Drawing = {
	readonly outer: readonly Decoration[];
	readonly inner: DecorationSet;
};

type Block = Read &
	Drawing & {
		/** What showed when its decorations were drawn. */
		readonly shown: string;
	};

/** What the selection shows by, apart from where it is. */
type Showing = {
	/** Whether the markers stay as they are until a press lets go. */
	readonly held: boolean;
	/** Whether the editor is read-only, so the selection shows nothing. */
	readonly reading: boolean;
	/** Whether the editor has the focus, without which the selection shows nothing. */
	readonly focused: boolean;
	/** A place in the visual whose source shows instead of its card, or -1. */
	readonly open: number;
};

type Drawn = Showing & {
	/** The blocks in the order of the text. Lines between them are blank. */
	readonly blocks: readonly Block[];
	readonly decorations: BlockDecorations;
	/** The selection the look last followed. */
	readonly from: number;
	readonly to: number;
	/** How many blocks the last change read. */
	readonly parsed: number;
	/** Every definition in the text, which each block was read with. */
	readonly defined: Definitions;
	/** Each footnote's number, by its identifier. */
	readonly numbers: Numbers;
};

type Numbers = ReadonlyMap<string, number>;

type Meta = {
	readonly held?: boolean;
	readonly reading?: boolean;
	readonly focused?: boolean;
	readonly open?: number;
};

const MARKDOWN = new PluginKey<Drawn>("markdownLook");

/** How many blocks the last change to the text read, to tell the look stays incremental. */
export const blocksParsed = (state: EditorState): number => MARKDOWN.getState(state)?.parsed ?? 0;

/** The length of the document's text, one line break between paragraphs. */
const textLength = (doc: ProseNode): number => doc.content.size - doc.childCount - 1;

const BLANK = /^\s*$/;
const FENCE_OPENS = /^ {0,3}(`{3,}|~{3,}|\${2,})(.*)$/;
const FENCE_CLOSES = /^ {0,3}(`{3,}|~{3,}|\${2,})\s*$/;

/** The fence a line opens, or null. A backtick fence's language can't hold a backtick. */
const opensFence = (line: string): string | null => {
	const match = FENCE_OPENS.exec(line);
	const fence = match?.[1];
	if (!fence) return null;
	return match[2]?.includes(fence[0]!) ? null : fence;
};

const closesFence = (line: string, fence: string): boolean => {
	const closer = FENCE_CLOSES.exec(line)?.[1];
	return closer !== undefined && closer[0] === fence[0] && closer.length >= fence.length;
};

// The tags whose HTML block in a reply a blank line doesn't end, which a
// note draws whole, such as `<details>`. Any other tag a reply draws also
// does on a line of its own.
const HTML_BLOCK_TAGS = new Set([
	"blockquote",
	"details",
	"div",
	"dl",
	"h1",
	"h2",
	"h3",
	"h4",
	"h5",
	"h6",
	"ol",
	"section",
	"summary",
	"table",
	"tbody",
	"tfoot",
	"thead",
	"ul",
]);
const TAG_ALONE = /^ {0,3}<[a-zA-Z][a-zA-Z0-9-]*(?:\s[^>]*)?>\s*$/;

/** HTML a line opens and doesn't close, which holds the lines after it until it does. */
type Hold = { readonly name: string; readonly depth: number };

const opensHold = (line: string): Hold | null => {
	const name = openingTag(line);
	if (!name || !(HTML_BLOCK_TAGS.has(name) || TAG_ALONE.test(line))) return null;
	const depth = depthOf(line, name);
	return depth > 0 ? { name, depth } : null;
};

/** Paragraphs of the text that make one block, before it's read. */
type Piece = { readonly from: number; readonly to: number; readonly text: string[] };

/**
 * Splits the paragraphs from `index`, at `pos`, into blocks. It starts
 * outside any fence or held HTML, and stops at a paragraph `stopAt` takes,
 * between blocks, or at the end. Gives the blocks and where it stopped. With
 * `html`, an HTML block such as `<details>` holds its blank lines through its
 * closing tag, as a fence does.
 */
const split = (
	doc: ProseNode,
	index: number,
	pos: number,
	stopAt: (pos: number) => boolean,
	html: boolean
): { pieces: Piece[]; stop: number } => {
	const pieces: Piece[] = [];
	let current: { from: number; text: string[] } | null = null;
	let fence: string | null = null;
	let hold: Hold | null = null;
	for (; index < doc.childCount; index++) {
		if (current === null && stopAt(pos)) return { pieces, stop: pos };
		const paragraph = doc.child(index);
		const line = paragraph.textContent;
		if (fence === null && hold === null && BLANK.test(line)) {
			if (current) pieces.push({ from: current.from, to: pos, text: current.text });
			current = null;
		} else {
			current ??= { from: pos, text: [] };
			current.text.push(line);
			if (hold !== null) {
				const depth: number = hold.depth + depthOf(line, hold.name);
				hold = depth > 0 ? { name: hold.name, depth } : null;
			} else if (fence !== null) {
				if (closesFence(line, fence)) fence = null;
			} else {
				fence = opensFence(line);
				if (fence === null && html) hold = opensHold(line);
			}
		}
		pos += paragraph.nodeSize;
	}
	if (current) pieces.push({ from: current.from, to: pos, text: current.text });
	return { pieces, stop: pos };
};

/** The selection, or one that touches nothing while the editor is read-only or unfocused. */
const selectionOf = (state: EditorState, { reading, focused }: Showing): Range =>
	reading || !focused ? { from: -1, to: -1 } : state.selection;

/**
 * Whether a block's source shows instead of its drawing: a visual's while
 * it's opened at `open`, any other while the selection touches it. All
 * three count from the block's start.
 */
const sourceShows = (preview: Preview, from: number, to: number, open: number): boolean =>
	preview.visual ? touches(preview.within, open, open) : touches(preview.within, from, to);

/** What shows in a block, to tell whether a new selection changes anything. */
const showingKey = (block: Read, selection: Range, open: number): string => {
	const { look } = block;
	const from = selection.from - block.from;
	const to = selection.to - block.from;
	const at = open - block.from;
	let key = "";
	for (const marker of look.markers) key += showing(marker, from, to) ? "1" : "0";
	for (const each of look.formulas) key += touches(each.within ?? each, from, to) ? "1" : "0";
	for (const preview of look.previews) key += sourceShows(preview, from, to, at) ? "1" : "0";
	for (const task of look.tasks) key += touches(task.within, from, to) ? "1" : "0";
	for (const each of look.footnotes) key += touches(each.within, from, to) ? "1" : "0";
	return key;
};

/** Every block's definitions as one, the first of each identifier winning. */
const definitionsOf = (blocks: readonly Read[]): Definitions => {
	const links = new Map<string, { label: string; url: string }>();
	const notes = new Map<string, string>();
	for (const { look } of blocks) {
		if (look.defines === NO_DEFINITIONS) continue;
		for (const [id, link] of look.defines.links) if (!links.has(id)) links.set(id, link);
		for (const [id, label] of look.defines.notes) if (!notes.has(id)) notes.set(id, label);
	}
	return links.size > 0 || notes.size > 0 ? { links, notes } : NO_DEFINITIONS;
};

/** What a block reads differently with, which is every label but not the addresses. */
const labelsKey = ({ links, notes }: Definitions): string =>
	`${[...links.values()].map((link) => link.label).join("\n")}\0${[...notes.values()].join("\n")}`;

/** The definitions some blocks hold, addresses and all, to tell whether a change moved any. */
const definesKey = (blocks: readonly Read[]): string =>
	blocks
		.filter((block) => block.look.defines !== NO_DEFINITIONS)
		.map(
			({ look }) =>
				`${labelsKey(look.defines)}\0${[...look.defines.links.values()].map((link) => link.url).join("\n")}`
		)
		.join("\u0001");

/** The footnote calls some blocks hold, in order, to tell whether a change moved any. */
const callsKey = (blocks: readonly Read[]): string =>
	blocks
		.flatMap((block) =>
			block.look.footnotes.map((each) => `${each.inDefinition ? "^" : ""}${each.id}`)
		)
		.join("\n");

/**
 * Each footnote's number, as a reply gives them: in the order of their first
 * calls, with the calls in footnotes' own text after the rest.
 */
const numbersOf = (blocks: readonly Read[]): Numbers => {
	const numbers = new Map<string, number>();
	for (const inDefinition of [false, true])
		for (const { look } of blocks)
			for (const each of look.footnotes)
				if (each.inDefinition === inDefinition && !numbers.has(each.id))
					numbers.set(each.id, numbers.size + 1);
	return numbers;
};

const numbersKey = (numbers: Numbers): string => [...numbers.keys()].join("\n");

const DEFINITION_LINE = /^[ >]*\[(\^?)([^\]\n]+)\]:[ \t]*(\S*)/;

/**
 * The definitions a text seems to hold, from a look at each line's start.
 * The blocks read with these, and read again if their own reading differs.
 */
const scanDefinitions = (pieces: readonly Piece[]): Definitions => {
	const links = new Map<string, { label: string; url: string }>();
	const notes = new Map<string, string>();
	for (const piece of pieces)
		for (const line of piece.text) {
			const found = line.includes("]:") ? DEFINITION_LINE.exec(line) : null;
			if (!found) continue;
			const [, note, label = "", url = ""] = found;
			const id = label
				.replace(/[\t\r\n ]+/g, " ")
				.trim()
				.toLowerCase();
			if (id === "") continue;
			if (note && !notes.has(id)) notes.set(id, label);
			if (!note && !links.has(id)) links.set(id, { label, url });
		}
	return links.size > 0 || notes.size > 0 ? { links, notes } : NO_DEFINITIONS;
};

// A press on a drawing is the widget's own, so ProseMirror doesn't place a
// caret of its own or hold the markers. The caret goes in to edit it.
const placeCaret = (view: EditorView, pos: number) => {
	view.dispatch(view.state.tr.setSelection(TextSelection.create(view.state.doc, pos)));
	view.focus();
};

/** The block a position is in, or null between blocks. */
const blockAt = (state: EditorState, pos: number): Block | null => {
	const blocks = MARKDOWN.getState(state)?.blocks ?? [];
	let low = 0;
	let high = blocks.length - 1;
	while (low <= high) {
		const middle = (low + high) >> 1;
		const block = blocks[middle]!;
		if (pos < block.from) high = middle - 1;
		else if (pos > block.to) low = middle + 1;
		else return block;
	}
	return null;
};

/** The code of every top-level fence that names a language, at its place in the document. */
export const codeFences = (state: EditorState): FenceCode[] => {
	const found: FenceCode[] = [];
	for (const block of MARKDOWN.getState(state)?.blocks ?? [])
		for (const each of block.look.code)
			found.push({ lang: each.lang, from: block.from + each.from, text: each.text });
	return found;
};

/** The index of a block's first line in the document. */
const firstLineOf = (doc: ProseNode, block: Block): number => doc.resolve(block.from).index(0);

const enterPreview = (view: EditorView, getPos: () => number | undefined, event: MouseEvent) => {
	event.preventDefault();
	const pos = getPos();
	if (!view.editable || pos === undefined) return;
	const block = blockAt(view.state, pos - 1);
	if (!block) return;
	const preview = block.look.previews.find((each) => block.from + each.within.to + 1 === pos);
	if (preview) placeCaret(view, block.from + preview.caret);
};

const enterFormula = (view: EditorView, getPos: () => number | undefined, event: MouseEvent) => {
	event.preventDefault();
	const pos = getPos();
	if (view.editable && pos !== undefined) placeCaret(view, pos + 1);
};

// A press on a task's box ticks it or clears it in the text, and leaves the
// caret where it was.
const tickTask = (view: EditorView, getPos: () => number | undefined, event: MouseEvent) => {
	event.preventDefault();
	const pos = getPos();
	if (!view.editable || pos === undefined) return;
	const box = view.state.doc.textBetween(pos, pos + 3);
	if (!/^\[[ xX]\]$/.test(box)) return;
	const mark = box[1] === " " ? "x" : " ";
	view.dispatch(view.state.tr.insertText(mark, pos + 1, pos + 2));
};

const taskBox = (task: Task, at: number): Decoration =>
	Decoration.widget(
		at,
		(view, getPos) => {
			const box = document.createElement("input");
			box.type = "checkbox";
			box.className = "md-task";
			box.checked = task.checked;
			box.disabled = !view.editable;
			box.setAttribute(
				"aria-label",
				task.checked ? translate("Done") : translate("Not done")
			);
			box.addEventListener("mousedown", (event) => tickTask(view, getPos, event));
			// The text decides whether it's ticked, not the box.
			box.addEventListener("click", (event) => event.preventDefault());
			return box;
		},
		{
			side: -1,
			key: `task:${task.checked}`,
			ignoreSelection: true,
			stopEvent: () => true,
		}
	);

const previewWidget = ({ kind, source, reply }: Preview & { kind: BlockKind }, at: number) =>
	Decoration.widget(
		at,
		(view, getPos) => {
			const dom = blockPreview(kind, source, reply);
			dom.addEventListener("mousedown", (event) => {
				// A press on a summary opens or shuts its details where they're drawn.
				const target = event.target instanceof Element ? event.target : null;
				if (kind === "html" && target?.closest("summary")) return;
				enterPreview(view, getPos, event);
			});
			return dom;
		},
		{
			side: -1,
			key: `block:${kind}:${reply ? "reply:" : ""}${source}`,
			ignoreSelection: true,
			stopEvent: () => true,
			destroy: removeBlockPreview,
		}
	);

// Each drawn visual's node, with where its block is in the document now,
// so the selection can mark the card it covers.
const visualRanges = new WeakMap<Element, () => Range | null>();

// A visual's card, which the host draws. A press in its page is the page's,
// so the editor leaves every event in it alone.
const visualWidget = (preview: Preview, at: number) =>
	Decoration.widget(
		at,
		(view, getPos) => {
			const { page, host } = preview.visual!;
			const dom = document.createElement("div");
			dom.className = "md-visual";
			dom.contentEditable = "false";
			// The widget sits just past the block's last line.
			const size = preview.within.to - preview.within.from;
			const range = (): Range | null => {
				const pos = getPos();
				return pos === undefined ? null : { from: pos - 1 - size, to: pos - 1 };
			};
			visualRanges.set(dom, range);
			host.draw(dom, page, {
				select: () => {
					const block = range();
					if (!block) return;
					const { doc } = view.state;
					view.dispatch(
						view.state.tr.setSelection(TextSelection.create(doc, block.from, block.to))
					);
					view.focus();
				},
				showSource: () => {
					const block = range();
					if (!block || !view.editable) return;
					const caret = block.from + preview.caret - preview.within.from;
					const { doc } = view.state;
					view.dispatch(
						view.state.tr
							.setSelection(TextSelection.create(doc, caret))
							.setMeta(MARKDOWN, { open: caret })
							.scrollIntoView()
					);
					view.focus();
				},
			});
			return dom;
		},
		{
			side: -1,
			key: `visual:${preview.visual!.key}`,
			ignoreSelection: true,
			stopEvent: () => true,
			destroy: (dom) => preview.visual!.host.remove(dom as HTMLElement),
		}
	);

/** Marks each drawn visual the selection covers whole, so its card shows it's selected. */
const markSelectedVisuals = (view: EditorView) => {
	const { from, to } = view.state.selection;
	for (const dom of view.dom.querySelectorAll(".md-visual")) {
		const range = visualRanges.get(dom)?.();
		const selected = !!range && from <= range.from && to >= range.to && from < to;
		if (selected !== dom.hasAttribute("data-selected"))
			dom.toggleAttribute("data-selected", selected);
	}
};

const formulaWidget = (formula: Formula, at: number) =>
	Decoration.widget(
		at,
		(view, getPos) => {
			const dom = formulaPreview(formula.source);
			dom.addEventListener("mousedown", (event) => enterFormula(view, getPos, event));
			return dom;
		},
		{
			side: -1,
			key: `formula:${formula.source}`,
			ignoreSelection: true,
			stopEvent: () => true,
		}
	);

// A footnote call as its number, raised, as a reply shows it.
const footnoteWidget = (number: number, at: number) =>
	Decoration.widget(
		at,
		(view, getPos) => {
			const dom = document.createElement("sup");
			dom.className = "md-footnote";
			dom.contentEditable = "false";
			dom.textContent = String(number);
			dom.addEventListener("mousedown", (event) => enterFormula(view, getPos, event));
			return dom;
		},
		{
			side: -1,
			key: `footnote:${number}`,
			ignoreSelection: true,
			stopEvent: () => true,
		}
	);

// A hidden `<br>` breaks the line where it is. A step onto its line shows
// the tag again, so the caret never has to cross the break.
const lineBreak = (at: number) =>
	Decoration.widget(at, () => document.createElement("br"), {
		side: -1,
		key: "br",
		ignoreSelection: true,
	});

/** A block's decorations while the selection is from `selection`, counted from its start. */
const decorate = (
	doc: ProseNode,
	block: Read,
	selection: Range,
	numbers: Numbers,
	open: number
): Drawing => {
	const { look } = block;
	const from = selection.from - block.from;
	const to = selection.to - block.from;
	const at = open - block.from;
	// The block on its own, so its look's positions are its positions.
	const own = doc.type.create(null, doc.content.cut(block.from, block.to));
	const inline = (range: Range, className: string) =>
		Decoration.inline(range.from, range.to, { class: className });
	const inner = look.runs.map((run) => inline(run, run.className));
	const folded = new Set<number>();
	for (const marker of look.markers) {
		const shown = showing(marker, from, to);
		inner.push(inline(marker, shown ? "md-marker" : "md-marker md-hidden"));
		if (!shown && marker.fold !== undefined) folded.add(marker.fold);
		if (!shown && marker.lineBreak) inner.push(lineBreak(marker.from));
	}
	for (const formula of look.formulas) {
		if (touches(formula.within ?? formula, from, to)) inner.push(inline(formula, "md-code"));
		else inner.push(inline(formula, "md-hidden"), formulaWidget(formula, formula.from));
	}
	for (const task of look.tasks)
		if (!touches(task.within, from, to))
			inner.push(inline(task, "md-hidden"), taskBox(task, task.from));
	for (const each of look.footnotes) {
		const number = numbers.get(each.id);
		if (number === undefined || touches(each.within, from, to))
			inner.push(inline(each, "md-marker"));
		else inner.push(inline(each, "md-hidden"), footnoteWidget(number, each.from));
	}
	const outer: Decoration[] = [];
	const drawn = look.previews.filter((preview) => !sourceShows(preview, from, to, at));
	for (const preview of drawn)
		outer.push(
			preview.kind === "visual"
				? visualWidget(preview, preview.within.to + 1)
				: previewWidget({ ...preview, kind: preview.kind }, preview.within.to + 1)
		);
	if (look.lines.size > 0 || drawn.length > 0 || folded.size > 0) {
		let pos = 0;
		for (let line = 0; line < own.childCount; line++) {
			const size = own.child(line).nodeSize;
			const hidden = drawn.some((each) => line >= each.first && line <= each.last);
			const classes = hidden ? "md-preview-source" : look.lines.get(line);
			const className = folded.has(line) && !hidden ? `${classes ?? ""} md-folded` : classes;
			if (className)
				outer.push(Decoration.node(pos, pos + size, { class: className.trim() }));
			pos += size;
		}
	}
	outer.sort((a, b) => a.from - b.from || a.to - b.to);
	return { outer, inner: DecorationSet.create(own, inner) };
};

/** A decoration moved by `shift`, of the same kind and spec. */
const shifted = (decoration: Decoration, shift: number): Decoration =>
	shift === 0
		? decoration
		: (decoration as unknown as { copy(from: number, to: number): Decoration }).copy(
				decoration.from + shift,
				decoration.to + shift
			);

// The decorations a block's lines take where it is now. Kept, so a block
// the text didn't move hands the view the same ones again.
const placed = new WeakMap<readonly Decoration[], { at: number; outer: Decoration[] }>();

const placeOuter = (block: Block): Decoration[] => {
	const kept = placed.get(block.outer);
	if (kept?.at === block.from) return kept.outer;
	const outer = block.outer.map((each) => shifted(each, block.from));
	placed.set(block.outer, { at: block.from, outer });
	return outer;
};

const DecorationSource = DecorationSet as unknown as new (
	local: Decoration[],
	children: unknown[]
) => DecorationSet;

/**
 * Every block's decorations, as the view asks for them. A plain set over a
 * long note holds every line's decorations at its top, and the view goes
 * through all of them for each line, so a key would cost the square of the
 * note's length. This one finds a line's block by halving, and hands the
 * view the block's own set.
 */
class BlockDecorations extends DecorationSource {
	private outer: Decoration[] | null = null;

	constructor(readonly blocks: readonly Block[]) {
		super([], []);
	}

	/** The decorations on the document's lines and between them. */
	localsInner(): Decoration[] {
		if (this.outer) return this.outer;
		const outer: Decoration[] = [];
		for (const block of this.blocks)
			if (block.outer.length > 0) for (const each of placeOuter(block)) outer.push(each);
		this.outer = outer;
		return outer;
	}

	override forChild(offset: number, child: ProseNode): ReturnType<DecorationSet["forChild"]> {
		const { blocks } = this;
		let low = 0;
		let high = blocks.length - 1;
		while (low <= high) {
			const middle = (low + high) >> 1;
			const block = blocks[middle]!;
			if (offset < block.from) high = middle - 1;
			else if (offset >= block.to) low = middle + 1;
			else return block.inner.forChild(offset - block.from, child);
		}
		return DecorationSet.empty;
	}

	eq(other: unknown): boolean {
		return other === this;
	}

	// The plugin draws again from its blocks on every change, so this is
	// never mapped.
	override map(): DecorationSet {
		return DecorationSet.empty;
	}

	override find(
		start?: number,
		end?: number,
		predicate?: (spec: { [key: string]: unknown }) => boolean
	): Decoration[] {
		const found: Decoration[] = [];
		for (const block of this.blocks) {
			if (start !== undefined && block.to < start) continue;
			if (end !== undefined && block.from > end) break;
			const inner = block.inner
				.find(
					start === undefined ? undefined : start - block.from,
					end === undefined ? undefined : end - block.from,
					predicate
				)
				.map((each) => shifted(each, block.from));
			const outer = placeOuter(block).filter(
				(each) =>
					(start === undefined || each.to >= start) &&
					(end === undefined || each.from <= end) &&
					(!predicate || predicate(each.spec as { [key: string]: unknown }))
			);
			found.push(...outer, ...inner);
		}
		return found;
	}
}

/** Reads the pieces into blocks, with the text's definitions. */
const readPieces = (
	pieces: readonly Piece[],
	options: MarkdownLookOptions,
	defined: Definitions
): Read[] =>
	pieces.map((piece) => {
		const text = piece.text.join("\n");
		const look =
			text.length <= MAX_BLOCK_LENGTH
				? markdownLook(text, options, defined)
				: options.visuals
					? largeVisualLook(piece.text, options.visuals)
					: NO_LOOK;
		return { from: piece.from, to: piece.to, lines: piece.text.length, look };
	});

/**
 * The look of a block past `MAX_BLOCK_LENGTH` that is one visual's fence
 * and nothing else, without reading the page as Markdown: its card, and
 * its lines as a fence's while its source shows. Any other block that long
 * stays plain.
 */
const largeVisualLook = (lines: readonly string[], host: VisualHost): Look => {
	const text = lines.join("\n");
	const page = fromVisualFence(text);
	const last = lines.length - 1;
	if (!page || page.html.trim() === "" || text !== text.trim() || last < 1) return NO_LOOK;
	const starts: number[] = [];
	lines.reduce((start, line) => {
		starts.push(start);
		return start + line.length + 2;
	}, 1);
	const within = { from: 1, to: starts[last]! + lines[last]!.length };
	const classes = new Map<number, string>();
	for (let line = 1; line < last; line++) classes.set(line, "md-code-line");
	classes.set(0, "md-code-line md-code-first md-fence");
	classes.set(last, "md-code-line md-code-last md-fence");
	const fold = (line: number): Marker => ({
		from: starts[line]!,
		to: starts[line]! + lines[line]!.length,
		within,
		fold: line,
	});
	return {
		...NO_LOOK,
		lines: classes,
		markers: [fold(0), fold(last)],
		fences: [{ first: 0, last: last - 1 }],
		previews: [
			{
				kind: "visual",
				source: page.html,
				first: 0,
				last,
				within,
				caret: starts[1]!,
				visual: { page, host, key: visualKey(text) },
			},
		],
	};
};

/** A block drawn for the selection. */
const drawBlock = (
	doc: ProseNode,
	block: Read,
	selection: Range,
	numbers: Numbers,
	open: number
): Block => ({
	...block,
	...decorate(doc, block, selection, numbers, open),
	shown: showingKey(block, selection, open),
});

/** Reads the whole text again. */
const readAll = (state: EditorState, options: MarkdownLookOptions, showing: Showing): Drawn => {
	const { doc } = state;
	const selection = selectionOf(state, showing);
	const base = { ...showing, from: selection.from, to: selection.to };
	if (textLength(doc) > options.maxLength)
		return {
			...base,
			blocks: [],
			decorations: new BlockDecorations([]),
			parsed: 0,
			defined: NO_DEFINITIONS,
			numbers: new Map(),
		};
	const { pieces } = split(doc, 0, 0, () => false, options.html);
	const scanned = scanDefinitions(pieces);
	let reads = readPieces(pieces, options, scanned);
	let parsed = reads.length;
	let defined = definitionsOf(reads);
	// A line that only looked like a definition, such as one in a fence.
	if (labelsKey(defined) !== labelsKey(scanned)) {
		reads = readPieces(pieces, options, defined);
		parsed += reads.length;
		defined = definitionsOf(reads);
	}
	const numbers = numbersOf(reads);
	const blocks = reads.map((read) => drawBlock(doc, read, selection, numbers, showing.open));
	return { ...base, blocks, decorations: new BlockDecorations(blocks), parsed, defined, numbers };
};

/** Where in the new document a transaction changed anything, or null. */
const changedRange = (tr: Transaction): Range | null => {
	let from = Infinity;
	let to = -Infinity;
	const { maps } = tr.mapping;
	maps.forEach((map, index) => {
		map.forEach((_oldStart, _oldEnd, start, end) => {
			const rest = tr.mapping.slice(index + 1);
			from = Math.min(from, rest.map(start, -1));
			to = Math.max(to, rest.map(end, 1));
		});
	});
	return from <= to ? { from, to } : null;
};

/**
 * Reads again the blocks a change touched, and moves the rest along. It
 * starts at the block before the change, since a line typed into a blank
 * one joins the blocks around it, and goes on past the change until a block
 * starts where one did before, outside a fence or held HTML.
 */
const readChange = (
	old: Drawn,
	tr: Transaction,
	state: EditorState,
	options: MarkdownLookOptions,
	held: boolean
): Drawn => {
	const { doc } = state;
	const changed = changedRange(tr);
	const showing = { held, reading: old.reading, focused: old.focused, open: old.open };
	if (textLength(doc) > options.maxLength || old.blocks.length === 0 || !changed)
		return readAll(state, options, showing);

	// Every old block, moved along. One the change emptied is gone.
	const moved: Block[] = [];
	const sizes = new Map<Block, number>();
	for (const block of old.blocks) {
		const from = tr.mapping.map(block.from, 1);
		const to = tr.mapping.map(block.to, -1);
		if (to <= from) continue;
		const next = from === block.from && to === block.to ? block : { ...block, from, to };
		sizes.set(next, block.to - block.from);
		moved.push(next);
	}

	const $changed = doc.resolve(Math.min(changed.from, doc.content.size));
	const lineBefore = Math.max(0, Math.min($changed.index(0), doc.childCount) - 1);
	const lineStart = $changed.posAtIndex(lineBefore, 0);
	let first = moved.findIndex((block) => block.to > lineStart);
	if (first === -1) first = moved.length;
	const start = Math.min(moved[first]?.from ?? lineStart, lineStart);

	// A block after the change that the change left alone, by where it starts.
	const after = new Map<number, number>();
	for (let index = first; index < moved.length; index++) {
		const block = moved[index]!;
		if (block.from >= changed.to && block.to - block.from === sizes.get(block))
			after.set(block.from, index);
	}
	const { pieces, stop } = split(
		doc,
		doc.resolve(start).index(0),
		start,
		(pos) => after.has(pos),
		options.html
	);
	const last = after.get(stop) ?? moved.length;

	const reads = readPieces(pieces, options, old.defined);
	const replaced = moved.slice(first, last);
	const before = moved.slice(0, first);
	const rest = moved.slice(last);
	let { defined, numbers } = old;
	// A definition added, dropped or renamed changes how other blocks read.
	if (definesKey(replaced) !== definesKey(reads)) {
		defined = definitionsOf([...before, ...reads, ...rest]);
		if (labelsKey(defined) !== labelsKey(old.defined)) return readAll(state, options, showing);
	}
	const selection = selectionOf(state, showing);
	let renumbered = false;
	if (callsKey(replaced) !== callsKey(reads)) {
		const next = numbersOf([...before, ...reads, ...rest]);
		renumbered = numbersKey(next) !== numbersKey(numbers);
		numbers = next;
	}
	const redraw = (block: Block): Block =>
		renumbered && block.look.footnotes.length > 0
			? drawBlock(doc, block, selection, numbers, old.open)
			: block;
	const blocks = [
		...before.map(redraw),
		...reads.map((read) => drawBlock(doc, read, selection, numbers, old.open)),
		...rest.map(redraw),
	];
	const from = tr.mapping.map(old.from);
	const to = tr.mapping.map(old.to);
	const decorations = new BlockDecorations(blocks);
	return { ...old, held, blocks, decorations, from, to, parsed: reads.length, defined, numbers };
};

/** The indexes of the blocks a range touches. */
const blocksTouching = (blocks: readonly Block[], range: Range): number[] => {
	let low = 0;
	let high = blocks.length;
	while (low < high) {
		const middle = (low + high) >> 1;
		if (blocks[middle]!.to < range.from) low = middle + 1;
		else high = middle;
	}
	const out: number[] = [];
	for (let index = low; index < blocks.length && blocks[index]!.from <= range.to; index++)
		out.push(index);
	return out;
};

/**
 * Draws again the blocks whose look the selection changed: the ones the
 * last selection touched, and the ones it touches now.
 */
const follow = (drawn: Drawn, state: EditorState): Drawn => {
	const selection = selectionOf(state, drawn);
	if (selection.from === drawn.from && selection.to === drawn.to) return drawn;
	const indexes = new Set([
		...blocksTouching(drawn.blocks, drawn),
		...blocksTouching(drawn.blocks, selection),
	]);
	return redraw(drawn, state, indexes, selection);
};

/** Draws again the blocks at `indexes` whose look differs for `selection`. */
const redraw = (
	drawn: Drawn,
	state: EditorState,
	indexes: Iterable<number>,
	selection: Range
): Drawn => {
	let blocks: Block[] | null = null;
	for (const index of indexes) {
		const block = drawn.blocks[index]!;
		const shown = showingKey(block, selection, drawn.open);
		if (shown === block.shown) continue;
		blocks ??= [...drawn.blocks];
		blocks[index] = {
			...block,
			...decorate(state.doc, block, selection, drawn.numbers, drawn.open),
			shown,
		};
	}
	return {
		...drawn,
		blocks: blocks ?? drawn.blocks,
		decorations: blocks ? new BlockDecorations(blocks) : drawn.decorations,
		from: selection.from,
		to: selection.to,
	};
};

/** The lines of the visual whose block holds `pos`, as a document range, or null. */
const visualAround = (blocks: readonly Block[], pos: number): Range | null => {
	const [index] = blocksTouching(blocks, { from: pos, to: pos });
	const block = index === undefined ? undefined : blocks[index];
	if (!block) return null;
	const at = pos - block.from;
	const preview = block.look.previews.find(
		(each) => each.visual && at >= each.within.from && at <= each.within.to
	);
	return preview
		? { from: block.from + preview.within.from, to: block.from + preview.within.to }
		: null;
};

/**
 * Where the visual whose source shows is now, or -1. A source stays open
 * while the selection touches its block. A caret the selection or the text
 * moved into a drawn visual's lines, such as one typing its closing fence,
 * opens it, since a caret can't sit in a card.
 */
const settleOpen = (drawn: Drawn, state: EditorState, open: number, moved: boolean): number => {
	if (drawn.reading) return -1;
	const { selection } = state;
	if (open >= 0) {
		const around = visualAround(drawn.blocks, open);
		if (around && touches(around, selection.from, selection.to)) return open;
	}
	if (moved && drawn.focused && selection.empty && visualAround(drawn.blocks, selection.from))
		return selection.from;
	return -1;
};

/** Draws again the visuals a change to the open one shows or hides. */
const reopen = (drawn: Drawn, state: EditorState, open: number): Drawn => {
	if (open === drawn.open) return drawn;
	const indexes = new Set<number>();
	for (const at of [drawn.open, open])
		if (at >= 0)
			for (const index of blocksTouching(drawn.blocks, { from: at, to: at }))
				indexes.add(index);
	// The selection the markers last followed, which a held press keeps.
	return redraw({ ...drawn, open }, state, indexes, drawn);
};

/**
 * Whether Enter adds a line here instead of sending: anywhere in a fenced
 * code block but its closing fence, and on through the text from a fence
 * that isn't closed yet.
 */
export const enterAddsLine = (state: EditorState): boolean => {
	const { $head } = state.selection;
	const block = blockAt(state, $head.pos);
	if (!block) return false;
	const line = $head.index(0) - firstLineOf(state.doc, block);
	return block.look.fences.some((fence) => line >= fence.first && line <= fence.last);
};

/**
 * Moves the caret into a drawn block next to it. Its lines take no room on
 * screen, so the browser's own step up or down would pass them by.
 */
const stepIntoPreview = (view: EditorView, event: KeyboardEvent): boolean => {
	const up = event.key === "ArrowUp";
	if (!up && event.key !== "ArrowDown") return false;
	if (event.shiftKey || event.altKey || event.metaKey || event.ctrlKey) return false;
	const { selection, doc } = view.state;
	if (!selection.empty) return false;
	const target = selection.$head.index(0) + (up ? -1 : 1);
	if (target < 0 || target >= doc.childCount) return false;
	const at = selection.$head.posAtIndex(target, 0) + 1;
	const block = blockAt(view.state, at);
	if (!block) return false;
	const line = target - firstLineOf(doc, block);
	const next = block.look.previews.find(
		(preview) => !preview.visual && (up ? preview.last === line : preview.first === line)
	);
	if (!next || !view.endOfTextblock(up ? "up" : "down")) return false;
	const pos = block.from + (up ? next.within.to : next.within.from);
	view.dispatch(view.state.tr.setSelection(TextSelection.create(doc, pos)).scrollIntoView());
	return true;
};

/** The lines of the visual block around `pos`, drawn or open, as a document range, or null. */
export const visualLinesAt = (state: EditorState, pos: number): Range | null => {
	const drawn = MARKDOWN.getState(state);
	return drawn ? visualAround(drawn.blocks, pos) : null;
};

/** The drawn visual on the document's `index`th line, as a document range, or null. */
const drawnVisualOn = (state: EditorState, index: number): Range | null => {
	const drawn = MARKDOWN.getState(state);
	if (!drawn || index < 0 || index >= state.doc.childCount) return null;
	const around = visualAround(drawn.blocks, state.doc.resolve(0).posAtIndex(index) + 1);
	return around && !touches(around, drawn.open, drawn.open) ? around : null;
};

/**
 * Keys that treat a drawn visual as one unit. An arrow onto it, or Backspace
 * or Delete into it, selects the whole block; an arrow from there steps off
 * it, and Enter opens its source. Its lines take no room on screen, so the
 * caret never stops inside them.
 */
const stepOverVisual = (view: EditorView, event: KeyboardEvent): boolean => {
	if (event.shiftKey || event.altKey || event.metaKey || event.ctrlKey) return false;
	const { state } = view;
	const { selection, doc } = state;
	const back = event.key === "ArrowUp" || event.key === "ArrowLeft";
	const forth = event.key === "ArrowDown" || event.key === "ArrowRight";
	const go = (from: number, to = from) => {
		view.dispatch(state.tr.setSelection(TextSelection.create(doc, from, to)).scrollIntoView());
		return true;
	};
	const line = selection.$from.index(0);
	const selected = drawnVisualOn(state, line);
	if (selected && selection.from === selected.from && selection.to === selected.to) {
		const last = doc.resolve(selected.to).index(0);
		if (back) return line > 0 ? go(selected.from - 2) : true;
		if (forth) return last + 1 < doc.childCount ? go(selected.to + 2) : true;
		if (event.key === "Enter" && view.editable) {
			const caret = doc.resolve(0).posAtIndex(line + 1) + 1;
			view.dispatch(
				state.tr
					.setSelection(TextSelection.create(doc, caret))
					.setMeta(MARKDOWN, { open: caret })
			);
			return true;
		}
		return false;
	}
	if (!selection.empty) return false;
	const { $head } = selection;
	const atStart = $head.parentOffset === 0;
	const atEnd = $head.parentOffset === $head.parent.content.size;
	const into = (target: number) => {
		const visual = drawnVisualOn(state, target);
		return visual ? go(visual.from, visual.to) : false;
	};
	if (event.key === "ArrowDown" && view.endOfTextblock("down")) return into(line + 1);
	if (event.key === "ArrowUp" && view.endOfTextblock("up")) return into(line - 1);
	if ((event.key === "ArrowRight" || event.key === "Delete") && atEnd) return into(line + 1);
	if ((event.key === "ArrowLeft" || event.key === "Backspace") && atStart) return into(line - 1);
	return false;
};

const SAFE_LINK = /^(https?:|mailto:)/i;

/** Opens the link under a Command-click, through the app's own way out to the browser. */
const openLink = (view: EditorView, event: MouseEvent): boolean => {
	const pos = view.posAtCoords({ left: event.clientX, top: event.clientY })?.pos;
	if (pos === undefined) return false;
	const block = blockAt(view.state, pos);
	const link = block?.look.links.find(
		(each) => pos >= block.from + each.from && pos <= block.from + each.to
	);
	if (!link) return false;
	// A reference link's address is in its definition, which any block can hold.
	const url =
		"url" in link ? link.url : MARKDOWN.getState(view.state)?.defined.links.get(link.ref)?.url;
	if (url === undefined || !SAFE_LINK.test(url)) return false;
	event.preventDefault();
	window.open(url, "_blank", "noreferrer");
	return true;
};

/** Draws the Markdown in the text as it will read. */
export const MarkdownLook = Extension.create<MarkdownLookOptions>({
	name: "markdownLook",
	addOptions: () => COMPOSER_LOOK,
	addProseMirrorPlugins() {
		const options = this.options;
		let timer: ReturnType<typeof setTimeout> | null = null;
		const stopTimer = () => {
			if (timer !== null) clearTimeout(timer);
			timer = null;
		};
		return [
			new Plugin<Drawn>({
				key: MARKDOWN,
				state: {
					init: (_, state) =>
						readAll(state, options, {
							held: false,
							reading: false,
							focused: false,
							open: -1,
						}),
					apply: (tr, old, _, state) => {
						const meta = tr.getMeta(MARKDOWN) as Meta | undefined;
						const held = meta?.held ?? old.held;
						const reading = meta?.reading ?? old.reading;
						const focused = meta?.focused ?? old.focused;
						const mapped =
							old.open >= 0 && tr.docChanged ? tr.mapping.map(old.open) : old.open;
						const asked = meta?.open ?? mapped;
						if (reading !== old.reading) {
							const drawn = follow(
								{ ...old, reading, focused, held, open: mapped },
								state
							);
							return reopen(drawn, state, reading ? -1 : asked);
						}
						const now =
							focused === old.focused && mapped === old.open
								? old
								: { ...old, focused, open: mapped };
						let drawn = tr.docChanged ? readChange(now, tr, state, options, held) : now;
						// A press that focuses the editor shows nothing new until it lets go.
						if (!held) drawn = follow(drawn, state);
						if (options.visuals) {
							const moved = tr.selectionSet || tr.docChanged;
							drawn = reopen(drawn, state, settleOpen(drawn, state, asked, moved));
						}
						return held === drawn.held ? drawn : { ...drawn, held };
					},
				},
				props: {
					decorations: (state) => MARKDOWN.getState(state)?.decorations ?? null,
					handleKeyDown: (view, event) =>
						(!!options.visuals && stepOverVisual(view, event)) ||
						stepIntoPreview(view, event),
					handleDOMEvents: {
						// The caret shows its line's markers only while you can type there.
						focus: (view) => {
							view.dispatch(view.state.tr.setMeta(MARKDOWN, { focused: true }));
							return false;
						},
						blur: (view) => {
							view.dispatch(view.state.tr.setMeta(MARKDOWN, { focused: false }));
							return false;
						},
						mousedown: (view, event) => {
							if (
								options.links &&
								event.button === 0 &&
								event.metaKey &&
								openLink(view, event)
							)
								return true;
							// A context menu can swallow the release.
							if (event.button !== 0 || event.ctrlKey) return false;
							stopTimer();
							view.dispatch(view.state.tr.setMeta(MARKDOWN, { held: true }));
							return false;
						},
					},
				},
				view: (view) => {
					const letGo = () => {
						if (!MARKDOWN.getState(view.state)?.held) return;
						stopTimer();
						timer = setTimeout(() => {
							timer = null;
							if (!view.isDestroyed)
								view.dispatch(view.state.tr.setMeta(MARKDOWN, { held: false }));
						}, MARKERS_HELD_MS);
					};
					// A read-only editor draws everything as it reads, wherever
					// a click leaves the selection.
					let editable = true;
					const checkEditable = () => {
						if (view.editable === editable) return;
						editable = view.editable;
						queueMicrotask(() => {
							if (!view.isDestroyed)
								view.dispatch(
									view.state.tr.setMeta(MARKDOWN, { reading: !view.editable })
								);
						});
					};
					checkEditable();
					// An editor can have the focus before this plugin hears of it.
					queueMicrotask(() => {
						if (!view.isDestroyed && view.hasFocus())
							view.dispatch(view.state.tr.setMeta(MARKDOWN, { focused: true }));
					});
					window.addEventListener("mouseup", letGo);
					// Dragging selected text ends with this, and no mouseup.
					window.addEventListener("dragend", letGo);
					if (options.visuals) markSelectedVisuals(view);
					return {
						update: () => {
							checkEditable();
							if (options.visuals) markSelectedVisuals(view);
						},
						destroy: () => {
							stopTimer();
							window.removeEventListener("mouseup", letGo);
							window.removeEventListener("dragend", letGo);
						},
					};
				},
			}),
		];
	},
});
