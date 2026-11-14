// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The regexes Shiki's JavaScript engine runs, made safe for WebKit.
//
// JavaScriptCore takes a regex whose first part is an optional group that
// starts with `^`, such as `(^[\t ]+)?(?=//)`, as anchored to where the
// search starts. A search from earlier in the line then never finds it.
// Grammars use that shape to take a comment's leading spaces, so in WebKit a
// TypeScript `//` comment after code drew as an operator and plain text.
// Other engines find the match. Writing `^` as a lookbehind for "no
// character before" means the same and keeps JavaScriptCore from anchoring.

import { defaultJavaScriptRegexConstructor } from "shiki/engine/javascript";

/** Matches where `^` does without the `m` flag: at the start of the input. */
const START = "(?<![^])";

/**
 * Rewrites each `^` outside a character class as {@link START}, except one
 * that opens the pattern, which JavaScriptCore anchors correctly.
 */
export const unanchor = (source: string, flags: string): string => {
	if (flags.includes("m") || !source.includes("^", 1)) return source;
	// With the `v` flag, classes nest.
	const nested = flags.includes("v");
	let out = "";
	let depth = 0;
	for (let i = 0; i < source.length; i++) {
		const c = source[i];
		if (c === "\\") {
			out += c + (source[i + 1] ?? "");
			i++;
			continue;
		}
		if (c === "[" && (depth === 0 || nested)) depth++;
		else if (c === "]" && depth > 0) depth--;
		else if (c === "^" && depth === 0 && i > 0) {
			out += START;
			continue;
		}
		out += c;
	}
	return out;
};

type Constructor = new (source: string, flags: string, options?: unknown) => RegExp;

/** Shiki's own regex for a grammar's pattern, with {@link unanchor} applied. */
export const regexFor = (pattern: string): RegExp => {
	const re = defaultJavaScriptRegexConstructor(pattern, { target: "auto" });
	const source = unanchor(re.source, re.flags);
	if (source === re.source) return re;
	// Some patterns come back as a RegExp subclass that keeps its options in
	// `rawOptions`. Building the same class with them keeps its emulation.
	const options = (re as RegExp & { rawOptions?: unknown }).rawOptions;
	return new (re.constructor as Constructor)(source, re.flags, options);
};
