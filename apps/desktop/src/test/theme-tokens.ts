// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The theme's tokens as the stylesheet writes them, for the tests that check
// each theme's colours. Each theme is the block for both themes with its own
// block over it, and a colour reads with every `var()` in it filled in.

import { parseColour, type Rgba } from "@maxhq/shared/theme/contrast";
import type { Theme } from "@maxhq/shared/theme/theme";

import css from "../styles/globals.css?raw";

export const DARK_SELECTOR = ':root,\n:root[data-theme="dark"]';
export const LIGHT_SELECTOR = ':root[data-theme="light"]';
export const BOTH_SELECTOR = ":root";

/** The custom properties in the block that opens with exactly `selector {`. */
export const themeBlock = (selector: string): Map<string, string> => {
	const opening = `\n${selector} {`;
	const start = css.indexOf(opening);
	if (start === -1) throw new Error(`No ${selector} block`);
	const body = css.slice(start + opening.length, css.indexOf("\n}", start));
	return new Map(
		[...body.matchAll(/(--[\w-]+):\s*([^;]+);/g)].map((match) => [match[1]!, match[2]!.trim()])
	);
};

const both = themeBlock(BOTH_SELECTOR);
const own = { dark: themeBlock(DARK_SELECTOR), light: themeBlock(LIGHT_SELECTOR) };

/** A token's value in a theme, with every `var()` in it filled in. */
export const tokenValue = (theme: Theme, name: string): string => {
	const value = own[theme].get(name) ?? both.get(name);
	if (value === undefined) throw new Error(`No ${name} in the ${theme} theme`);
	return value.replace(/var\((--[\w-]+)\)/g, (_, inner: string) => tokenValue(theme, inner));
};

/** A colour written with tokens, read in a theme. */
export const colourIn = (theme: Theme, text: string): Rgba => {
	const filled = text.replace(/var\((--[\w-]+)\)/g, (_, name: string) => tokenValue(theme, name));
	const colour = parseColour(filled.startsWith("--") ? tokenValue(theme, filled) : filled);
	if (!colour) throw new Error(`Couldn't read ${text} in the ${theme} theme`);
	return colour;
};
