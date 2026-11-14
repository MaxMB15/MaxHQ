// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A visual is one self-contained web page with a title. As text it's a
// fenced block whose info string is `html visual`, with an optional
// `title="..."` where `\"` and `\\` stand for a quote and a backslash:
//
//     ```html visual title="Token use by day"
//     <canvas id="c" height="150"></canvas>
//     ```
//
// Plain `html` stays code, since agents show HTML source that way all the
// time. Anything that reads only the first word, as GitHub does, shows the
// block as highlighted HTML. The fence is longer than the longest run of
// backticks in the page, as CommonMark allows, so the page can hold any.

/** A visual's page and title. An empty title means it has none. */
export type Visual = { readonly html: string; readonly title: string };

// The word after the language, then any attributes. A title's quotes hold
// anything but a bare quote or backslash.
const META = /^visual(?:\s+|$)/;
const TITLE = /(?:^|\s)title="((?:[^"\\]|\\.)*)"/;

/**
 * The title of a fence whose language is `lang` and whose info string goes
 * on with `meta`, or null when the fence isn't a visual.
 */
export const visualFence = (
	lang: string | null | undefined,
	meta: string | null | undefined
): { readonly title: string } | null => {
	if (lang?.toLowerCase() !== "html") return null;
	const rest = (meta ?? "").trim();
	if (!META.test(rest)) return null;
	const quoted = TITLE.exec(rest.replace(META, ""))?.[1];
	return { title: quoted === undefined ? "" : quoted.replace(/\\(.)/g, "$1") };
};

// A backtick can't go in a backtick fence's info string, and a line break
// would end it.
const cleanTitle = (title: string): string =>
	title
		.replace(/[\r\n]+/g, " ")
		.replaceAll("`", "'")
		.trim();

/** The longest run of backticks in `text`. */
const longestRun = (text: string): number =>
	Math.max(0, ...(text.match(/`+/g) ?? []).map((run) => run.length));

/** A visual as a fenced block, which reads back as the same visual. */
export const toVisualFence = ({ html, title }: Visual): string => {
	const fence = "`".repeat(Math.max(3, longestRun(html) + 1));
	const named = cleanTitle(title);
	const info = named
		? `html visual title="${named.replace(/["\\]/g, (c) => `\\${c}`)}"`
		: "html visual";
	return `${fence}${info}\n${html}\n${fence}`;
};

const OPENING = /^(`{3,}|~{3,})([^\n]*)\n/;

/**
 * The visual a text holds when the whole text, give or take blank space
 * around it, is one visual's fenced block. Anything else gives null.
 */
export const fromVisualFence = (text: string): Visual | null => {
	const block = text.replace(/\r\n?/g, "\n").trim();
	const opening = OPENING.exec(block);
	if (!opening) return null;
	const [line, fence = "", info = ""] = opening;
	if (fence[0] === "`" && info.includes("`")) return null;
	const [lang = "", ...meta] = info.trim().split(/\s+/);
	const visual = visualFence(lang, meta.join(" "));
	if (!visual) return null;
	const body = block.slice(line.length);
	// The block closes on the last line and nowhere before it.
	const lines = body.split("\n");
	const closes = (row: string) => {
		const run = row.trim();
		return run.length >= fence.length && [...run].every((c) => c === fence[0]);
	};
	const last = lines.pop() ?? "";
	if (!closes(last) || lines.some(closes)) return null;
	return { html: lines.join("\n"), title: visual.title };
};

// A fence that opens a block at the top level, and its info string.
const OPENS = /^ {0,3}(`{3,}|~{3,})([^\n]*)$/;

/** Whether `line` closes a block opened by `fence`. */
const closesFence = (line: string, fence: string): boolean => {
	const run = line.trim();
	return (
		line.length - line.trimStart().length < 4 &&
		run.length >= fence.length &&
		[...run].every((c) => c === fence[0])
	);
};

/**
 * The finished visuals in a Markdown text, in order, as its blocks at the
 * top level hold them. A text that doesn't name one costs a single search.
 */
export const visualsIn = (markdown: string): Visual[] => {
	if (!markdown.includes("visual")) return [];
	const lines = markdown.replace(/\r\n?/g, "\n").split("\n");
	const found: Visual[] = [];
	for (let at = 0; at < lines.length; at++) {
		const opening = OPENS.exec(lines[at] ?? "");
		if (!opening) continue;
		const [, fence = "", info = ""] = opening;
		const end = lines.findIndex((line, i) => i > at && closesFence(line, fence));
		if (end < 0) break;
		const [lang = "", ...meta] = info.trim().split(/\s+/);
		const visual =
			fence[0] === "`" && info.includes("`") ? null : visualFence(lang, meta.join(" "));
		if (visual) found.push({ html: lines.slice(at + 1, end).join("\n"), title: visual.title });
		at = end;
	}
	return found;
};
