// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Where each group of a match starts and ends, for a JavaScript engine
// without the regex `d` flag. Shiki's JavaScript engine reads those places
// from `match.indices`, which only the flag gives. Without it, each group's
// place is found by searching for its text, from where the group before it
// starts, since a group opens after the ones before it. A group repeated or
// inside a lookaround can land on the wrong place, which only costs a few
// characters their colour.

/** Each group's start and end in the input, undefined for a group that took no part. */
export type Indices = ([number, number] | undefined)[];

/** Whether this engine's regexes take the `d` flag. */
export const HAS_INDICES = (() => {
	try {
		new RegExp("", "d");
		return true;
	} catch {
		return false;
	}
})();

/** Each group's place in `match`, found by its text. The whole match's is exact. */
export const guessIndices = (match: RegExpExecArray): Indices => {
	const start = match.index;
	const input = match.input;
	const indices: Indices = [[start, start + match[0].length]];
	let from = start;
	for (let group = 1; group < match.length; group++) {
		const text = match[group];
		if (text === undefined) {
			indices.push(undefined);
			continue;
		}
		let at = input.indexOf(text, from);
		// Text before the group's place, as a lookbehind takes.
		if (at < 0) at = input.lastIndexOf(text, from);
		if (at < 0) at = from;
		indices.push([at, at + text.length]);
		from = at;
	}
	return indices;
};

/** `regex`, its matches given `indices` worked out by {@link guessIndices}. */
export const withGuessedIndices = <R extends RegExp>(regex: R): R => {
	// The regex's own exec, which a subclass may override.
	const exec = regex.exec;
	regex.exec = function (this: RegExp, input: string) {
		const match = exec.call(this, input);
		if (match && !match.indices) {
			Object.defineProperty(match, "indices", {
				value: guessIndices(match),
				writable: true,
				configurable: true,
			});
		}
		return match;
	};
	return regex;
};
