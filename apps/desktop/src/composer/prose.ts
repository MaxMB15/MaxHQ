// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Which parts of a message are prose worth spell checking. A message is
// Markdown with code, commands, paths and links mixed in, and none of those
// should get a red underline. Everything that isn't prose turns into spaces,
// so the checker's offsets still line up with the text.

import { FENCE_OPENER } from "@maxhq/shared/pills/draft";

const URL = /\b(?:[a-z][a-z0-9+.-]*:\/\/|www\.|mailto:)\S+/gi;
// A Markdown link's target, right after its label's closing bracket.
const LINK_TARGET = /\]\([^()\s]*\)/g;
const LEADING = /^[("'“‘[{<*_]+/;
const TRAILING = /[)"'”’\]}>.,;:!?*_]+$/;

const blank = (text: string): string => " ".repeat(text.length);

/**
 * Whether a word looks like code or a name rather than prose: a flag, a
 * mention, a path, a file name, a symbol, a camelCase name or anything
 * with a digit.
 */
export const isCodeLike = (word: string): boolean => {
	if (/^[-@/~]/.test(word)) return true;
	const core = word.replace(LEADING, "").replace(TRAILING, "");
	if (core === "") return false;
	return /[_./\\:#=<>{}[\]$|`^~%&+*]/.test(core) || /\d/.test(core) || /.\p{Lu}/u.test(core);
};

/** Blanks inline code spans, which a run of backticks opens and the same run closes. */
const blankInlineCode = (line: string): string => {
	let out = "";
	let at = 0;
	while (at < line.length) {
		const open = line.indexOf("`", at);
		if (open === -1) break;
		let ticks = open;
		while (line[ticks] === "`") ticks++;
		const run = line.slice(open, ticks);
		let close = line.indexOf(run, ticks);
		while (close !== -1 && line[close + run.length] === "`") {
			let skip = close;
			while (line[skip] === "`") skip++;
			close = line.indexOf(run, skip);
		}
		if (close === -1) {
			out += line.slice(at, ticks);
			at = ticks;
			continue;
		}
		out += line.slice(at, open) + blank(line.slice(open, close + run.length));
		at = close + run.length;
	}
	return out + line.slice(at);
};

/**
 * Blanks the code-like words and links in one line of prose. A link keeps
 * its label, whose last word would otherwise run into the target and pass
 * for code.
 */
const blankLine = (line: string): string =>
	blankInlineCode(line)
		.replace(LINK_TARGET, (target) => "]" + blank(target.slice(1)))
		.replace(URL, blank)
		.replace(/\S+/g, (word) => (isCodeLike(word) ? blank(word) : word));

/**
 * The prose in each line of a message, as one string per line with the rest
 * blanked. Lines in a fenced code block, fences included, come back blank.
 * Lines outside `from` to `to` come back empty, with only their fences read,
 * so a long note pays for the lines it shows.
 */
export const proseLines = (lines: readonly string[], from = 0, to = lines.length): string[] => {
	let fence: string | null = null;
	return lines.map((line, index) => {
		const marker = FENCE_OPENER.exec(line)?.[1];
		const wanted = index >= from && index < to;
		if (fence !== null) {
			if (marker && marker[0] === fence[0] && marker.length >= fence.length) fence = null;
			return wanted ? blank(line) : "";
		}
		if (marker) {
			fence = marker;
			return wanted ? blank(line) : "";
		}
		return wanted ? blankLine(line) : "";
	});
};

/** The prose in a message, with everything else blanked. */
export const prose = (text: string): string => proseLines(text.split("\n")).join("\n");
