// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The libraries the app carries for visuals, by the file a page names after
// `maxhq:lib/`. Metro makes each module the library's text (see
// metro.config.js and visual-libraries.js, whose list this matches). Each is
// read the first time a page names it, so a chart doesn't read Mermaid.

type Text = () => string;

// Metro needs each name written out, and a require, which runs only when
// called, where an import would read every library as the app starts.
/* eslint-disable @typescript-eslint/no-require-imports */
const LIBRARIES: ReadonlyMap<string, Text> = new Map([
	["chart.js", () => require("maxhq-visual-lib/chart.js") as string],
	["d3.js", () => require("maxhq-visual-lib/d3.js") as string],
	["mermaid.js", () => require("maxhq-visual-lib/mermaid.js") as string],
	["katex.js", () => require("maxhq-visual-lib/katex.js") as string],
]);
/* eslint-enable @typescript-eslint/no-require-imports */

/** The names of the libraries the app carries. */
export const BUNDLED_LIBRARIES = [...LIBRARIES.keys()];

const read = new Map<string, string>();

/** One library's text, or undefined for one the app doesn't carry. */
export const bundledLibrary = (name: string): string | undefined => {
	let text = read.get(name);
	if (text === undefined) {
		text = LIBRARIES.get(name)?.();
		if (text !== undefined) read.set(name, text);
	}
	return text;
};

// All of them, for anything that goes through the lot.
const readAll = () => new Map(BUNDLED_LIBRARIES.map((name) => [name, bundledLibrary(name) ?? ""]));

const LAZY: ReadonlyMap<string, string> = {
	get: bundledLibrary,
	has: (name) => LIBRARIES.has(name),
	size: LIBRARIES.size,
	forEach: (each, self) => readAll().forEach(each, self),
	entries: () => readAll().entries(),
	keys: () => LIBRARIES.keys(),
	values: () => readAll().values(),
	[Symbol.iterator]: () => readAll()[Symbol.iterator](),
};

/** Each library the app carries, by name, read when it's first asked for. */
export const bundledLibraries = (): ReadonlyMap<string, string> => LAZY;
