// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { contrast, over } from "@maxhq/shared/theme/contrast";
import type { Theme } from "@maxhq/shared/theme/theme";
import { describe, expect, it } from "vitest";

import {
	BOTH_SELECTOR,
	colourIn,
	DARK_SELECTOR,
	LIGHT_SELECTOR,
	themeBlock,
} from "@/test/theme-tokens";

const THEMES: readonly Theme[] = ["dark", "light"];

// WCAG's least for text, and for the parts of a control and the marks that
// carry a state.
const TEXT = 4.5;
const PART = 3;

const SURFACES = [
	"--background",
	"--card",
	"--popover",
	"--subpanel",
	"--sidebar",
	"--hover-card",
	"--secondary",
	"--accent",
];

/** A colour as it shows: one that lets light through sits on the pane. */
const shown = (theme: Theme, text: string) => {
	const colour = colourIn(theme, text);
	return colour.a < 1 ? over(colour, colourIn(theme, "--background")) : colour;
};

const tint = (status: string, share: number, under: string) =>
	`color-mix(in srgb, var(${status}) ${share}%, var(${under}))`;

type Pair = readonly [text: string, ground: string, least: number];

const PAIRS: readonly Pair[] = [
	// Text in each of its weights on every surface it sits on.
	...[
		"--foreground",
		"--foreground-strong",
		"--secondary-foreground",
		"--muted-foreground",
	].flatMap((text) => SURFACES.map((ground): Pair => [text, ground, TEXT])),
	["--foreground", "--canvas", TEXT],
	["--foreground-strong", "--selected", TEXT],
	["--secondary-foreground", "--selected", TEXT],
	["--muted-foreground", "--selected", TEXT],
	...["--background", "--card", "--subpanel"].map((ground): Pair => ["--terminal", ground, TEXT]),
	// Hints, placeholders and line numbers.
	...SURFACES.map((ground): Pair => ["--subtle-foreground", ground, PART]),
	// Filled buttons and badges, and links and errors written in an accent.
	["--primary-foreground", "--primary", TEXT],
	// The composer's Run button, and its Shell tag on the composer.
	["--shell-foreground", "--shell", TEXT],
	["--shell", "--subpanel", TEXT],
	["--status-ready-foreground", "--status-ready", TEXT],
	...["--background", "--card", "--popover"].flatMap((ground): Pair[] => [
		["--primary", ground, TEXT],
		["--destructive", ground, TEXT],
	]),
	// A status's own text on the tint of its colour, and on plain panes.
	...(
		[
			["--status-ready-text", "--status-ready"],
			["--status-busy-text", "--status-busy"],
			["--status-connecting-text", "--status-connecting"],
			["--status-error-text", "--status-error"],
		] as const
	).flatMap(([text, status]): Pair[] => [
		[text, "--background", TEXT],
		[text, "--card", TEXT],
		[text, tint(status, 15, "--background"), TEXT],
		[text, tint(status, 15, "--card"), TEXT],
	]),
	// The status marks and the focus ring against what they sit on.
	...[
		"--status-busy",
		"--status-ready",
		"--status-warm",
		"--status-linked",
		"--status-connecting",
		"--status-error",
		"--status-off",
		"--ring",
	].flatMap((mark) =>
		["--sidebar", "--background", "--card", "--hover-card"].map((ground): Pair => [
			mark,
			ground,
			PART,
		])
	),
	// Code on the panel it shows in.
	...[
		"--code-text",
		"--code-comment",
		"--code-keyword",
		"--code-constant",
		"--code-variable",
		"--code-function",
		"--code-tag",
		"--code-string",
		"--code-invalid",
	].map((code): Pair => [code, "--subpanel", TEXT]),
	// A diagram's series on its panel, and the words drawn on them.
	...Array.from({ length: 8 }, (_, i): Pair[] => [
		[`--series-${i}`, "--subpanel", PART],
		["--on-series", `--series-${i}`, TEXT],
	]).flat(),
	// A diff's marks and line numbers on the tint of its added and removed lines.
	["--diff-added", tint("--status-ready", 8, "--subpanel"), TEXT],
	["--diff-removed", tint("--destructive", 8, "--subpanel"), TEXT],
	// Find's matches.
	["--find-match-text", "--find-match-bg", TEXT],
	["--find-active-text", "--find-active-bg", TEXT],
];

// The dark theme keeps the mockups' values, and these few fall a little short
// there. Each name is `text on ground`.
const DARK_SHORT = new Set([
	"--subtle-foreground on --card",
	"--subtle-foreground on --popover",
	"--subtle-foreground on --sidebar",
	"--subtle-foreground on --hover-card",
	"--subtle-foreground on --secondary",
	"--subtle-foreground on --accent",
	"--status-off on --sidebar",
	"--status-off on --card",
	"--status-off on --hover-card",
	"--find-match-text on --find-match-bg",
]);

describe("the themes", () => {
	const dark = themeBlock(DARK_SELECTOR);
	const light = themeBlock(LIGHT_SELECTOR);
	const both = themeBlock(BOTH_SELECTOR);

	it("gives every dark token a light value", () => {
		expect([...dark.keys()].filter((name) => !light.has(name))).toEqual([]);
	});

	it("gives no light token that dark lacks", () => {
		expect([...light.keys()].filter((name) => !dark.has(name))).toEqual([]);
	});

	it("keeps the tokens for both themes out of each theme's own block", () => {
		expect([...both.keys()].filter((name) => dark.has(name) || light.has(name))).toEqual([]);
	});

	// Mermaid works its colours out with a library that reads hex and rgb,
	// not oklch.
	it.each(THEMES)("writes what Mermaid reads in hex in the %s theme", (theme) => {
		const read = [
			"--foreground",
			"--muted-foreground",
			"--subtle-foreground",
			"--subpanel",
			"--popover",
			"--card",
			"--hover-card",
			"--hover-card-border",
			"--border",
			"--on-series",
			...Array.from({ length: 8 }, (_, i) => `--series-${i}`),
		];
		const block = theme === "dark" ? dark : light;
		expect(read.filter((name) => !/^#[0-9a-f]{6}$/.test(block.get(name) ?? ""))).toEqual([]);
	});

	for (const theme of THEMES)
		it.each(PAIRS)(`in the ${theme} theme, %s on %s reads at %s:1`, (text, ground, least) => {
			const under = shown(theme, ground);
			const ratio = contrast(over(colourIn(theme, text), under), under);
			if (theme === "dark" && DARK_SHORT.has(`${text} on ${ground}`))
				expect(ratio).toBeLessThan(least);
			else expect(ratio).toBeGreaterThanOrEqual(least);
		});

	it("lists only dark pairs it checks", () => {
		const checked = new Set(PAIRS.map(([text, ground]) => `${text} on ${ground}`));
		expect([...DARK_SHORT].filter((name) => !checked.has(name))).toEqual([]);
	});
});
