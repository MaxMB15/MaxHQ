// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The libraries the app carries for visuals, by the file a page names after
// `maxhq:lib/`. Metro makes each module the library's text (see
// metro.config.js and visual-libraries.js, whose list this matches), and
// none is read until a visual first opens.

type Text = () => string;

// Metro needs each name written out, and a require, which runs only when
// called, where an import would read every library as the app starts.
/* eslint-disable @typescript-eslint/no-require-imports */
const LIBRARIES: Readonly<Record<string, Text>> = {
	"chart.js": () => require("maxhq-visual-lib/chart.js") as string,
	"d3.js": () => require("maxhq-visual-lib/d3.js") as string,
};
/* eslint-enable @typescript-eslint/no-require-imports */

/** The names of the libraries the app carries. */
export const BUNDLED_LIBRARIES = Object.keys(LIBRARIES);

let read: ReadonlyMap<string, string> | null = null;

/** Each library the app carries, by name, with its text. */
export const bundledLibraries = (): ReadonlyMap<string, string> => {
	read ??= new Map(Object.entries(LIBRARIES).map(([name, text]) => [name, text()]));
	return read;
};
