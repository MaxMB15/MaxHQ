// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What Markdown reads beyond the grammar remark-gfm and remark-math give it:
// callouts in a quote, the `$` that aren't math, and the raw HTML a reply may
// show. Agents' text isn't trusted, so raw HTML goes through an allowlist.

import { PLAIN_DOLLAR, restoreDollars } from "@maxhq/shared/agent-view/markdown";
import { visualFence } from "@maxhq/shared/visuals/fence";
import { defaultSchema, type Options as Schema } from "rehype-sanitize";

/** What a callout can look like. */
export type AlertKind =
	| "note"
	| "abstract"
	| "info"
	| "todo"
	| "tip"
	| "success"
	| "question"
	| "important"
	| "warning"
	| "caution"
	| "failure"
	| "danger"
	| "bug"
	| "example"
	| "quote";

/**
 * Each word a callout can start with: GitHub's five, and Obsidian's with its
 * other names. `crates/core/src/quote_span.rs` lists the same words.
 */
const ALERT_WORDS: Readonly<Record<string, AlertKind>> = {
	note: "note",
	abstract: "abstract",
	summary: "abstract",
	tldr: "abstract",
	info: "info",
	todo: "todo",
	tip: "tip",
	hint: "tip",
	success: "success",
	check: "success",
	done: "success",
	question: "question",
	help: "question",
	faq: "question",
	important: "important",
	warning: "warning",
	attention: "warning",
	caution: "caution",
	failure: "failure",
	fail: "failure",
	missing: "failure",
	danger: "danger",
	error: "danger",
	bug: "bug",
	example: "example",
	quote: "quote",
	cite: "quote",
};

// Just enough of a syntax tree node to rewrite it.
type TreeNode = {
	type: string;
	value?: string;
	children?: TreeNode[];
	data?: {
		hName?: string;
		hProperties?: Record<string, unknown>;
		hChildren?: TreeNode[];
	};
	[field: string]: unknown;
};

const DOLLAR_FIELDS = ["value", "url", "title", "alt", "lang", "meta", "identifier", "label"];

const plainDollars = (node: TreeNode) => {
	for (const field of DOLLAR_FIELDS) {
		const held = node[field];
		if (typeof held === "string" && held.includes(PLAIN_DOLLAR))
			node[field] = restoreDollars(held);
	}
	node.children?.forEach(plainDollars);
	node.data?.hChildren?.forEach(plainDollars);
};

/** A remark plugin that puts back each `$` `guardDollars` swapped before parsing. */
export const remarkPlainDollars = () => plainDollars;

// `[!WORD]`, what folds it in Obsidian, then a title on the rest of the line.
const ALERT_LINE = /^\[!([A-Za-z]+)\][+-]?[ \t]*([^\n]*)(?:\n|$)/;

/** A callout's first line, as a quote's first text starts with it. */
export type AlertLine = {
	readonly kind: AlertKind;
	readonly title: string;
	/** The length of the whole line, its line break included. */
	readonly length: number;
	/** The length of its `[!WORD]`, with what folds it. */
	readonly mark: number;
};

/** The callout a quote's first text opens, or null when it opens none. */
export const alertLine = (text: string): AlertLine | null => {
	const line = ALERT_LINE.exec(text);
	const word = line?.[1]!.toLowerCase();
	// Only its own words, since any object also answers to `constructor`.
	if (!line || !word || !Object.hasOwn(ALERT_WORDS, word)) return null;
	const mark = word.length + 3 + (/^[+-]/.test(line[0].slice(word.length + 3)) ? 1 : 0);
	return { kind: ALERT_WORDS[word]!, title: line[2]!.trim(), length: line[0].length, mark };
};

const alertIn = (quote: TreeNode) => {
	const paragraph = quote.children?.[0];
	const first = paragraph?.type === "paragraph" ? paragraph.children?.[0] : undefined;
	if (first?.type !== "text" || first.value === undefined || !paragraph) return;
	const line = alertLine(first.value);
	if (!line) return;
	const { kind, title } = line;
	first.value = first.value.slice(line.length);
	if (first.value === "") paragraph.children?.shift();
	// A soft break can't start what's left, since the text held the line break.
	if (paragraph.children?.length === 0) quote.children?.shift();
	quote.data = { ...quote.data, hProperties: { ...quote.data?.hProperties, dataAlert: kind } };
	quote.children?.unshift({
		type: "alertTitle",
		children: title ? [{ type: "text", value: title }] : [],
		data: { hName: "div", hProperties: { dataAlertTitle: kind } },
	});
};

const alerts = (node: TreeNode) => {
	if (node.type === "blockquote") alertIn(node);
	node.children?.forEach(alerts);
};

/** A remark plugin that reads a quote starting with `[!NOTE]` as a callout. */
export const remarkAlerts = () => alerts;

// Where a node starts and ends in the text, as remark gives it.
type Place = {
	readonly start?: { readonly offset?: number };
	readonly end?: { readonly offset?: number };
};

// A line that closes a fence, behind any quote or list marks it sits in.
const CLOSING = /^[\s>]*(`{3,}|~{3,})\s*$/;

const visuals = (node: TreeNode, text: string) => {
	const fence =
		node.type === "code" ? visualFence(node.lang as string, node.meta as string) : null;
	if (fence) {
		const place = node.position as Place | undefined;
		const written = text.slice(place?.start?.offset ?? 0, place?.end?.offset ?? text.length);
		const opening = /^[\s>]*(`{3,}|~{3,})/.exec(written)?.[1] ?? "```";
		const lines = written.split("\n");
		const closing = lines.length > 1 ? CLOSING.exec(lines.at(-1) ?? "")?.[1] : undefined;
		const closed =
			closing !== undefined && closing[0] === opening[0] && closing.length >= opening.length;
		node.data = {
			...node.data,
			hProperties: {
				...node.data?.hProperties,
				dataVisual: fence.title,
				...(closed ? {} : { dataOpen: "true" }),
			},
		};
	}
	node.children?.forEach((child) => visuals(child, text));
};

/**
 * A remark plugin that marks each fence whose info string is `html visual`
 * with its title, and one that hasn't closed yet, as one streaming in hasn't.
 */
export const remarkVisualFences = () => (tree: TreeNode, file: { value: unknown }) =>
	visuals(tree, String(file.value));

const without = <Entry>(list: readonly Entry[] | null | undefined, ...names: string[]): Entry[] =>
	(list ?? []).filter((entry) => typeof entry !== "string" || !names.includes(entry));

/** The tags a reply can show. `crates/core/src/quote_span.rs` lists the same. */
const SHOWN_TAGS: readonly string[] = [
	...without(defaultSchema.tagNames, "picture", "source").filter(
		(name): name is string => typeof name === "string"
	),
	"mark",
];
const SHOWN = new Set(SHOWN_TAGS);

// These stand with no pair, because they're void or a page closes them itself.
const NEEDS_NO_PAIR = new Set([
	"br",
	"hr",
	"img",
	"input",
	"li",
	"p",
	"dt",
	"dd",
	"tr",
	"td",
	"th",
	"thead",
	"tbody",
	"tfoot",
	"rp",
	"rt",
]);

/** Whether a reply draws a tag that takes a closing tag, such as `<mark>` or `<details>`. */
export const drawsPairedTag = (name: string): boolean =>
	SHOWN.has(name) && !NEEDS_NO_PAIR.has(name);

// A comment, what starts a declaration, a `</` with no name, or a tag with
// its name, then what follows up to its end. A `>` in quotes doesn't end it.
const HTML_MARKUP =
	/<!--[\s\S]*?(?:-->|$)|<[?!]|<\/(?![a-zA-Z])|<(\/?)([a-zA-Z][^\s/>]*)(?:"[^"]*"|'[^']*'|[^'">])*(>?)/g;

type Markup = { readonly node: TreeNode; readonly at: number; readonly length: number };

/**
 * A remark plugin that turns raw HTML that wouldn't draw into its own text:
 * a tag off the list, as in `Vec<String>` or "add a <style> block", and a tag
 * with no pair, which would hold or drop what follows it. Whether a tag runs
 * is still up to `htmlSchema`, and this only keeps a sentence as it was
 * written. Tags pair up across the block, since Markdown can sit between them.
 */
export const remarkLiteralTags = () => (tree: TreeNode) => {
	const open: (Markup & { readonly name: string })[] = [];
	const literal: Markup[] = [];
	const read = (node: TreeNode) => {
		for (const match of node.type === "html" ? (node.value ?? "").matchAll(HTML_MARKUP) : []) {
			const [text, closing, tag, ended] = match;
			if (text.startsWith("<!--")) continue;
			const name = tag?.toLowerCase();
			const markup = { node, at: match.index, length: text.length };
			if (name === undefined || !ended || !SHOWN.has(name)) literal.push(markup);
			else if (NEEDS_NO_PAIR.has(name)) continue;
			else if (!closing) open.push({ ...markup, name });
			else {
				const pair = open.findLastIndex((entry) => entry.name === name);
				if (pair < 0) literal.push(markup);
				else literal.push(...open.splice(pair).slice(1));
			}
		}
		node.children?.forEach(read);
	};
	read(tree);
	// From the last, so each one's place holds as those before it change.
	const lastFirst = [...literal, ...open].sort((one, other) => other.at - one.at);
	for (const { node, at, length } of lastFirst) {
		const value = node.value ?? "";
		const text = value.slice(at, at + length).replaceAll("<", "&lt;");
		node.value = value.slice(0, at) + text + value.slice(at + length);
	}
};

/**
 * The HTML a reply can show, which is GitHub's list with one message's ids.
 * Its own `id`s all start with `prefix`, from the footnotes, and a tag can't
 * name any other, since that could take the place of something on the page.
 * Pictures and links go on to the same components Markdown's do, so a
 * picture's `src` is left to `MarkdownMedia`.
 */
export const htmlSchema = (prefix: string): Schema => {
	const { attributes, protocols } = defaultSchema;
	const own: [string, RegExp] = ["id", new RegExp(`^${prefix}`)];
	return {
		...defaultSchema,
		tagNames: [...SHOWN_TAGS],
		protocols: { ...protocols, src: null },
		clobber: [],
		clobberPrefix: "",
		attributes: {
			...attributes,
			"*": without(attributes?.["*"], "id", "name"),
			a: [...(attributes?.a ?? []), own],
			li: [...(attributes?.li ?? []), own],
			code: [
				["className", /^language-./, "math-inline", "math-display"],
				"dataVisual",
				"dataOpen",
			],
			blockquote: [...(attributes?.blockquote ?? []), "dataAlert"],
			div: [...(attributes?.div ?? []), "dataAlertTitle"],
		},
	};
};
