// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Turns a tool's output, with its terminal colour codes, into runs of text
// that each carry one style. Only SGR codes (ESC [ ... m) style the text.
// Every other escape, such as a cursor move or a window title, is dropped,
// and a carriage return keeps what was written after it, as a terminal
// shows a progress bar that redraws its line.

/** How one run of text looks. Colours are `#rrggbb`, or null for the default. */
export type AnsiStyle = {
	readonly fg: string | null;
	readonly bg: string | null;
	readonly bold: boolean;
	readonly dim: boolean;
	readonly italic: boolean;
	readonly underline: boolean;
};

/** A run of text in one style. Newlines stay in the text. */
export type AnsiSpan = { readonly text: string; readonly style: AnsiStyle };

/** Text with no styling. */
export const PLAIN: AnsiStyle = {
	fg: null,
	bg: null,
	bold: false,
	dim: false,
	italic: false,
	underline: false,
};

// The 16 basic colours, tuned to read on a dark background.
const BASIC = [
	"#3b4252",
	"#f0616d",
	"#3fb37f",
	"#e8c547",
	"#4692f6",
	"#a07cf0",
	"#3fc1c9",
	"#d8dee9",
	"#5b6472",
	"#ff8a93",
	"#8fe3b5",
	"#f5d97a",
	"#7fb4ff",
	"#c3a6ff",
	"#7fdfe5",
	"#ffffff",
] as const;

const hex = (n: number): string => n.toString(16).padStart(2, "0");
const rgb = (r: number, g: number, b: number): string => `#${hex(r)}${hex(g)}${hex(b)}`;

/** The colour for an xterm 256-colour index. */
export const paletteColor = (index: number): string => {
	if (index < 16) return BASIC[index]!;
	if (index < 232) {
		const n = index - 16;
		const step = (v: number) => (v === 0 ? 0 : 55 + v * 40);
		return rgb(step(Math.floor(n / 36)), step(Math.floor(n / 6) % 6), step(n % 6));
	}
	const grey = 8 + (index - 232) * 10;
	return rgb(grey, grey, grey);
};

const byte = (value: number | undefined): number | null =>
	value !== undefined && Number.isInteger(value) && value >= 0 && value <= 255 ? value : null;

// Reads an extended colour (38 or 48) starting at `codes[i]`, which is 5 or 2.
// Answers the colour, or undefined when it's malformed, and how many codes it took.
const extended = (codes: readonly number[], i: number): [string | undefined, number] => {
	if (codes[i] === 5) {
		const index = byte(codes[i + 1]);
		return [index === null ? undefined : paletteColor(index), 2];
	}
	if (codes[i] === 2) {
		const [r, g, b] = [byte(codes[i + 1]), byte(codes[i + 2]), byte(codes[i + 3])];
		return [r === null || g === null || b === null ? undefined : rgb(r, g, b), 4];
	}
	return [undefined, 1];
};

/** The style after one SGR sequence's codes. An empty list resets, as `ESC [ m` does. */
export const applySgr = (style: AnsiStyle, codes: readonly number[]): AnsiStyle => {
	if (codes.length === 0) return PLAIN;
	let next = style;
	for (let i = 0; i < codes.length; i++) {
		const code = codes[i]!;
		if (code === 0) next = PLAIN;
		else if (code === 1) next = { ...next, bold: true };
		else if (code === 2) next = { ...next, dim: true };
		else if (code === 3) next = { ...next, italic: true };
		else if (code === 4) next = { ...next, underline: true };
		else if (code === 22) next = { ...next, bold: false, dim: false };
		else if (code === 23) next = { ...next, italic: false };
		else if (code === 24) next = { ...next, underline: false };
		else if (code >= 30 && code <= 37) next = { ...next, fg: BASIC[code - 30]! };
		else if (code >= 90 && code <= 97) next = { ...next, fg: BASIC[code - 90 + 8]! };
		else if (code >= 40 && code <= 47) next = { ...next, bg: BASIC[code - 40]! };
		else if (code >= 100 && code <= 107) next = { ...next, bg: BASIC[code - 100 + 8]! };
		else if (code === 39) next = { ...next, fg: null };
		else if (code === 49) next = { ...next, bg: null };
		else if (code === 38 || code === 48) {
			const [color, used] = extended(codes, i + 1);
			if (color !== undefined)
				next = code === 38 ? { ...next, fg: color } : { ...next, bg: color };
			i += used;
		}
	}
	return next;
};

// CSI (ESC [ params final), OSC (ESC ] ... BEL or ESC \), any other
// two-byte escape, and a lone ESC at the end. Built from the escape's
// character code, since a control character in a pattern reads as a mistake.
const ESC = String.fromCharCode(27);
const BEL = String.fromCharCode(7);
const ESCAPE = new RegExp(
	`${ESC}\\[([0-9;:?]*)([@-~])|${ESC}\\][^${BEL}${ESC}]*(?:${BEL}|${ESC}\\\\)?|${ESC}[@-_]?`,
	"g"
);

// Keeps what a line shows after its last carriage return.
const redrawn = (text: string): string =>
	text
		.replace(/\r\n/g, "\n")
		.split("\n")
		.map((line) => line.slice(line.lastIndexOf("\r") + 1))
		.join("\n");

const sameStyle = (a: AnsiStyle, b: AnsiStyle): boolean =>
	a.fg === b.fg &&
	a.bg === b.bg &&
	a.bold === b.bold &&
	a.dim === b.dim &&
	a.italic === b.italic &&
	a.underline === b.underline;

/** The output as styled runs, next to each other runs merged when they look the same. */
export const parseAnsi = (output: string): AnsiSpan[] => {
	const text = output.includes("\r") ? redrawn(output) : output;
	const spans: AnsiSpan[] = [];
	const push = (part: string, style: AnsiStyle) => {
		if (part === "") return;
		const last = spans.at(-1);
		if (last && sameStyle(last.style, style)) {
			spans[spans.length - 1] = { text: last.text + part, style };
		} else spans.push({ text: part, style });
	};
	let style = PLAIN;
	let from = 0;
	for (const match of text.matchAll(ESCAPE)) {
		push(text.slice(from, match.index), style);
		from = match.index + match[0].length;
		if (match[2] === "m") {
			const params = match[1] ?? "";
			const codes =
				params === "" ? [] : params.split(/[;:]/).map((p) => (p === "" ? 0 : Number(p)));
			style = applySgr(style, codes);
		}
	}
	push(text.slice(from), style);
	return spans;
};

/**
 * Runs in blocks of at most `lines` lines, for a view that can't draw one
 * block too tall. Each run keeps its style across a break.
 */
export const ansiBlocks = (spans: readonly AnsiSpan[], lines: number): AnsiSpan[][] => {
	const blocks: AnsiSpan[][] = [];
	let block: AnsiSpan[] = [];
	let left = lines;
	for (const { text, style } of spans) {
		let from = 0;
		for (let at = text.indexOf("\n"); at >= 0; at = text.indexOf("\n", at + 1)) {
			if (--left > 0) continue;
			if (at > from) block.push({ text: text.slice(from, at), style });
			blocks.push(block);
			block = [];
			left = lines;
			from = at + 1;
		}
		if (from < text.length) block.push({ text: text.slice(from), style });
	}
	if (block.length > 0) blocks.push(block);
	return blocks;
};

/** The output with every escape taken out, for a copy or a search. */
export const stripAnsi = (output: string): string =>
	parseAnsi(output)
		.map((span) => span.text)
		.join("");
