// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { contrast, over } from "@maxhq/shared/theme/contrast";
import { describe, expect, it } from "vitest";

import { colourIn, tokenValue } from "@/test/theme-tokens";

const FAMILIES = [
	"file",
	"quote",
	"agent",
	"note",
	"git-open",
	"git-draft",
	"git-merged",
	"git-closed",
	"failed",
	"unresolved",
];

const THEMES = ["dark", "light"] as const;

describe("pill colors", () => {
	// A pill sits in the composer and in the sent bubble.
	const grounds = { composer: "--subpanel", bubble: "--secondary" };

	for (const theme of THEMES)
		for (const family of FAMILIES)
			for (const [where, ground] of Object.entries(grounds))
				it(`reads at 4.5:1 for ${family} in the ${theme} set on the ${where}`, () => {
					const text = colourIn(theme, `--pill-${family}-text`);
					const fill = over(
						colourIn(theme, `--pill-${family}-bg`),
						colourIn(theme, ground)
					);
					expect(contrast(text, fill)).toBeGreaterThanOrEqual(4.5);
				});

	it("keeps the note's yellow apart from find's amber", () => {
		const hueOf = (text: string) => Number(/ ([\d.]+)(?: \/ [\d.]+)?\)$/.exec(text)![1]);
		for (const theme of THEMES) {
			const note = hueOf(tokenValue(theme, "--pill-note-text"));
			for (const find of ["--find-match-bg", "--find-active-bg"])
				expect(Math.abs(note - hueOf(tokenValue(theme, find)))).toBeGreaterThanOrEqual(10);
		}
	});

	it("gives every family the same three tokens in both sets", () => {
		for (const theme of THEMES)
			for (const family of FAMILIES)
				for (const part of ["text", "bg", "border"])
					expect(() => tokenValue(theme, `--pill-${family}-${part}`)).not.toThrow();
	});
});
