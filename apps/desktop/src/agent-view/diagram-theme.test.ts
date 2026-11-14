// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it } from "vitest";

import { setThemeChoice } from "@/app/theme";
import { themeBlock, tokenValue } from "@/test/theme-tokens";
import type { Theme } from "@maxhq/shared/theme/theme";

import { drawDiagram, drawnDiagram } from "./diagram-render";
import { diagramTheme } from "./diagram-theme";

/** Puts a theme's tokens on the page, as its stylesheet would. */
const showTheme = (theme: Theme) => {
	setThemeChoice(theme);
	for (const name of themeBlock(':root[data-theme="light"]').keys())
		document.documentElement.style.setProperty(name, tokenValue(theme, name));
};

afterEach(() => setThemeChoice("system"));

describe("a diagram's colours", () => {
	it.each(["light", "dark"] as const)("come from the %s theme's tokens", (theme) => {
		const settings = diagramTheme((name) => tokenValue(theme, name), theme);
		expect(settings.darkMode).toBe(theme === "dark");
		expect(settings.background).toBe(tokenValue(theme, "--subpanel"));
		for (let i = 0; i < 8; i++)
			expect(settings[`pie${i + 1}`]).toBe(tokenValue(theme, `--series-${i}`));
		expect(settings.pieSectionTextColor).toBe(tokenValue(theme, "--on-series"));
	});

	it("draws again in a new theme, and keeps each theme's drawing", async () => {
		const source = 'pie\n "a": 1\n "b": 2';
		const pictured = (url: string) => decodeURIComponent(url).toLowerCase();
		showTheme("dark");
		const dark = await drawDiagram(source);
		expect(dark.state === "drawn" && pictured(dark.url)).toContain(
			tokenValue("dark", "--series-0")
		);
		showTheme("light");
		expect(drawnDiagram(source)).toBeUndefined();
		const light = await drawDiagram(source);
		expect(light.state === "drawn" && pictured(light.url)).toContain(
			tokenValue("light", "--series-0")
		);
		showTheme("dark");
		expect(drawnDiagram(source)).toBe(dark);
	}, 40_000);
});
