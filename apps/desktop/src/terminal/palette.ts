// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A terminal's colours, read from the theme's tokens in globals.css, so a
// terminal draws in the look the window shows and follows it when it
// switches. The names are the ones a terminal's theme takes.

/** The 16 colours, each as the token names it after `--terminal-`. */
const NAMED = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"] as const;

type Basic = (typeof NAMED)[number];
type Bright = `bright${Capitalize<Basic>}`;

/** Every colour a terminal draws with, as CSS colours. */
export type TerminalPalette = Readonly<
	Record<Basic | Bright | "background" | "foreground" | "cursor" | "selectionBackground", string>
>;

const capital = <T extends string>(word: T) =>
	(word.charAt(0).toUpperCase() + word.slice(1)) as Capitalize<T>;

/** Each palette entry with the token it reads. */
const TOKENS: readonly (readonly [keyof TerminalPalette, string])[] = [
	["background", "--terminal-background"],
	["foreground", "--terminal-foreground"],
	["cursor", "--terminal-cursor"],
	["selectionBackground", "--terminal-selection"],
	...NAMED.map((name) => [name, `--terminal-${name}`] as const),
	...NAMED.map(
		(name) => [`bright${capital(name)}` as Bright, `--terminal-bright-${name}`] as const
	),
];

/** The palette the page's theme gives now. */
export const readPalette = (root: Element = document.documentElement): TerminalPalette => {
	const style = getComputedStyle(root);
	return Object.fromEntries(
		TOKENS.map(([key, token]) => [key, style.getPropertyValue(token).trim()])
	) as TerminalPalette;
};

/** Calls `listener` each time the page switches theme, until the returned call. */
export const watchPalette = (listener: (palette: TerminalPalette) => void): (() => void) => {
	const root = document.documentElement;
	const observer = new MutationObserver(() => listener(readPalette(root)));
	observer.observe(root, { attributes: true, attributeFilter: ["data-theme"] });
	return () => observer.disconnect();
};
