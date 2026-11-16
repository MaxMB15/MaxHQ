// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Replies and plans render as Markdown one top-level block at a time, so a
// reply streaming in only renders its last block again. The desktop and the
// phone split the same way.
//
// A streaming text is split again each time it grows. Where a block ends
// hangs only on the line or two after it, so every block before the last two
// is closed by then. The split keeps those and lexes again only from the
// second to last, which the last can still join, as a rule line does when it
// grows into text.

import { Lexer } from "marked";

// A link or footnote definition, which any block in the text can name.
const DEFINITION = /^[ >]*\[[^\]\n]+\]:/m;
// Math between `$$` lines, whose blank lines aren't breaks between blocks.
const MATH_LINE = /^ {0,3}\$\$/m;

/** One top-level token: where it starts in the text, its kind and its source. */
type Piece = { readonly start: number; readonly type: string; readonly raw: string };

/** A text and its top-level tokens, spaces included. */
type Split = { readonly text: string; readonly pieces: readonly Piece[] };

/** The last few texts split, so each one's next version picks up from it. */
const recent: Split[] = [];
const RECENT = 8;

// Raw HTML such as `<details>` holds the blocks between its opening and its
// closing tag, and a comment holds nothing.
const holdsBlocks = (pieces: readonly Piece[]): boolean =>
	pieces.some((piece) => piece.type === "html" && !piece.raw.trimStart().startsWith("<!--"));

/**
 * The top-level tokens of `text` from `start` on. Only the blocks count
 * here, so it skips lexing what's inside them.
 */
const lexFrom = (text: string, start: number): Piece[] => {
	let at = start;
	return new Lexer().blockTokens(text.slice(start)).map(({ type, raw }) => {
		const piece = { start: at, type, raw };
		at += raw.length;
		return piece;
	});
};

/** The tokens of `text`, from the longest recent text it extends when there is one. */
const piecesOf = (text: string): Piece[] => {
	const before = recent
		.filter((split) => text.startsWith(split.text))
		.reduce<Split | undefined>(
			(a, b) => (a && a.text.length >= b.text.length ? a : b),
			undefined
		);
	if (before?.text === text) return [...before.pieces];
	const from = before?.pieces.filter((piece) => piece.type !== "space").at(-2);
	if (!before || !from) return lexFrom(text, 0);
	return [...before.pieces.slice(0, before.pieces.indexOf(from)), ...lexFrom(text, from.start)];
};

/** Keeps a text's tokens for its next version, if they cover the whole text. */
const remember = (text: string, pieces: Piece[]) => {
	const last = pieces.at(-1);
	if ((last ? last.start + last.raw.length : 0) !== text.length) return;
	const at = recent.findIndex((split) => text.startsWith(split.text));
	if (at >= 0) recent.splice(at, 1);
	else if (recent.length >= RECENT) recent.shift();
	recent.push({ text, pieces });
};

/**
 * The top-level blocks of a Markdown text, each as its source. A text whose
 * blocks depend on each other stays whole, which only costs it rendering
 * all of it again as it streams.
 */
export const splitBlocks = (text: string): string[] => {
	if (DEFINITION.test(text) || MATH_LINE.test(text)) return [text];
	// The lexer reads each carriage return as a newline, so the blocks come from that text.
	const source = text.includes("\r") ? text.replace(/\r\n|\r/g, "\n") : text;
	try {
		const pieces = piecesOf(source);
		remember(source, pieces);
		if (holdsBlocks(pieces)) return [text];
		return pieces.filter((piece) => piece.type !== "space").map((piece) => piece.raw);
	} catch {
		return [text];
	}
};

/**
 * Stands in for a `$` that isn't math while Markdown parses. It's one code
 * unit, as the `$` is, so every offset in the text still holds.
 */
export const PLAIN_DOLLAR = "﷐";

const WORD = /[\p{L}\p{N}]/u;
const SPACE = /\s/;

const isWord = (c: string | undefined): boolean => c !== undefined && WORD.test(c);
const isSpace = (c: string | undefined): boolean => c === undefined || SPACE.test(c);

// Where the math that a `$` at `open` opens ends, or -1 when it opens none.
// Math can't hold another `$`, so the next one closes it or nothing does.
const closingDollar = (text: string, open: number): number => {
	for (let at = open + 1; at < text.length; at++) {
		const c = text[at];
		if (c === "\\") at++;
		else if (c === "\n" && /^\s*\n/.test(text.slice(at + 1, at + 256))) return -1;
		else if (c === "$") return isSpace(text[at - 1]) || isWord(text[at + 1]) ? -1 : at;
	}
	return -1;
};

/**
 * `text` with each `$` that isn't around math swapped for `PLAIN_DOLLAR`.
 * Math sits in a pair with no space just inside it and no letter or digit
 * just outside it, so `$x^2$` is math, and `$5 and $10`, `$5-$10`, `US$5` and
 * `$HOME` stay text. `$$` is left to the parser.
 */
export const guardDollars = (text: string): string => {
	if (!text.includes("$")) return text;
	let guarded = "";
	let copied = 0;
	const plain = (at: number) => {
		guarded += `${text.slice(copied, at)}${PLAIN_DOLLAR}`;
		copied = at + 1;
	};
	for (let at = 0; at < text.length; at++) {
		const c = text[at];
		if (c === "\\") {
			at++;
		} else if (c === "$") {
			if (text[at + 1] === "$") {
				while (text[at + 1] === "$") at++;
				continue;
			}
			const canOpen = !isWord(text[at - 1]) && !isSpace(text[at + 1]);
			const closing = canOpen ? closingDollar(text, at) : -1;
			if (closing === -1) plain(at);
			else at = closing;
		}
	}
	return guarded + text.slice(copied);
};

/** Puts each `$` that `guardDollars` swapped back. */
export const restoreDollars = (text: string): string => text.replaceAll(PLAIN_DOLLAR, "$");

/** A fenced code block: the language its fence names, its code, and whether it has closed. */
export type CodeFence = {
	readonly lang: string | null;
	readonly code: string;
	/** False while its closing fence hasn't come, as for a block streaming in. */
	readonly closed: boolean;
};

// A fence's opening line: its indent, its marks, and what follows them.
const FENCE_OPENING = /^( {0,3})(`{3,}|~{3,})(.*)$/;

// A fence's closing line, its marks in the first group.
const FENCE_CLOSING = /^ {0,3}(`{3,}|~{3,})[ \t]*$/;

/**
 * The code block a top-level block is, when it's one fence and nothing more,
 * or null. The language is the first word after the marks, and each line of
 * code gives up as much of its indent as the opening line had.
 */
export const codeFenceOf = (block: string): CodeFence | null => {
	const lines = block.replace(/\r\n?/g, "\n").split("\n");
	const opening = FENCE_OPENING.exec(lines[0] ?? "");
	if (!opening) return null;
	const [, indent = "", marks = "", info = ""] = opening;
	// A backtick fence's info can't hold a backtick.
	if (marks[0] === "`" && info.includes("`")) return null;
	const code: string[] = [];
	let closed = false;
	let at = 1;
	for (; at < lines.length; at++) {
		const line = lines[at] ?? "";
		const run = FENCE_CLOSING.exec(line)?.[1];
		if (run && run[0] === marks[0] && run.length >= marks.length) {
			closed = true;
			break;
		}
		const spaces = /^ */.exec(line)?.[0].length ?? 0;
		code.push(line.slice(Math.min(spaces, indent.length)));
	}
	// Anything but blank lines after it makes the block more than a fence.
	if (lines.slice(at + 1).some((line) => line.trim() !== "")) return null;
	// A block still coming in may end in the break before its next line.
	if (!closed && code.at(-1) === "") code.pop();
	return { lang: info.trim().split(/\s+/)[0] || null, code: code.join("\n"), closed };
};
