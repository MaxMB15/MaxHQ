// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import codeTheme from "shiki/themes/github-dark-default.mjs";
import { describe, expect, it } from "vitest";

import { tokenValue } from "@/test/theme-tokens";

import { CODE_COLORS, tokenize } from "./highlight-worker";

const TOKEN = /^var\((--code-[\w-]+)\)$/;

describe("highlighted code's colours", () => {
	it("stand each of the code theme's colours for a token both themes have", () => {
		const used = [
			codeTheme.colors?.["editor.foreground"],
			...(codeTheme.tokenColors ?? []).map((rule) => rule.settings.foreground),
		].filter((color): color is string => color !== undefined);
		for (const color of new Set(used.map((c) => c.toLowerCase()))) {
			const name = TOKEN.exec(CODE_COLORS[color] ?? "")?.[1];
			expect(name, color).toBeDefined();
			expect(() => tokenValue("light", name!)).not.toThrow();
		}
	});

	it("keep the code theme's colours in the dark theme", () => {
		for (const [color, value] of Object.entries(CODE_COLORS)) {
			const name = TOKEN.exec(value)![1]!;
			if (name !== "--code-text") expect(tokenValue("dark", name), name).toBe(color);
		}
		expect(tokenValue("dark", "--code-text")).toBe(
			codeTheme.colors?.["editor.foreground"]?.toLowerCase()
		);
	});

	it("come out of the highlighter as tokens", async () => {
		const lines = await tokenize({
			id: 1,
			code: '// a note\nconst one = f("two", 3);',
			lang: "typescript",
		});
		const colors = new Set(lines!.flat().map((token) => token.color));
		expect([...colors].filter((color) => !TOKEN.test(color ?? ""))).toEqual([]);
		expect(colors).toContain("var(--code-comment)");
		expect(colors).toContain("var(--code-string)");
	});
});
