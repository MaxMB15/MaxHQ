// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { DIAGRAM_CSS, diagramTheme } from "./diagram-theme";
import { drawingSize, failureReason, mathSettings, mermaidSettings, svgUrl } from "./drawing";

// Each token's value is its own name, so a setting shows where it came from.
const named = (name: string) => name;

describe("a diagram's theme", () => {
	it("takes its colours from the tokens it's given", () => {
		const theme = diagramTheme(named, "dark");
		expect(theme.darkMode).toBe(true);
		expect(theme.background).toBe("--subpanel");
		expect(theme.primaryTextColor).toBe("--foreground");
		for (let i = 0; i < 8; i++) {
			expect(theme[`pie${i + 1}`]).toBe(`--series-${i}`);
			expect(theme[`cScale${i}`]).toBe(`--series-${i}`);
		}
		expect(theme.pieSectionTextColor).toBe("--on-series");
		expect(diagramTheme(named, "light").darkMode).toBe(false);
	});
});

describe("Mermaid's settings", () => {
	it("keep the strict mode and the look out of a diagram's reach", () => {
		const theme = diagramTheme(named, "dark");
		const settings = mermaidSettings(theme);
		expect(settings).toMatchObject({
			startOnLoad: false,
			securityLevel: "strict",
			htmlLabels: false,
			theme: "base",
			themeCSS: DIAGRAM_CSS,
		});
		expect(settings.themeVariables).toBe(theme);
		for (const key of ["securityLevel", "theme", "themeCSS", "htmlLabels", "secure"])
			expect(settings.secure).toContain(key);
	});
});

describe("KaTeX's settings", () => {
	it("trust nothing and give up only on what's too deep to read", () => {
		expect(mathSettings(true, "mathml", "red")).toEqual({
			displayMode: true,
			output: "mathml",
			throwOnError: false,
			errorColor: "red",
			trust: false,
			strict: "ignore",
			maxSize: 20,
		});
	});
});

describe("a drawing's size", () => {
	it("comes from its largest width first, then its width, then its box", () => {
		expect(drawingSize({ maxWidth: "200px", width: "100%", viewBox: "0 0 400 100" })).toEqual({
			width: 200,
			height: 50,
		});
		expect(drawingSize({ maxWidth: "", width: "300.2", viewBox: "0,0,600,300" })).toEqual({
			width: 301,
			height: 151,
		});
		expect(drawingSize({ maxWidth: "", width: "100%", viewBox: "-8 -8 120 60" })).toEqual({
			width: 120,
			height: 60,
		});
	});

	it("is none without a box to keep its shape", () => {
		expect(drawingSize({ maxWidth: "200px", width: null, viewBox: null })).toBeNull();
		expect(drawingSize({ maxWidth: "", width: "100", viewBox: "0 0 0 10" })).toBeNull();
		expect(drawingSize({ maxWidth: "", width: null, viewBox: "0 0 x 10" })).toBeNull();
	});
});

describe("a drawing's address", () => {
	it("gives the picture its size", () => {
		const url = svgUrl('<svg xmlns="http://www.w3.org/2000/svg"><g/></svg>', 20, 10);
		expect(url.startsWith("data:image/svg+xml;charset=utf-8,")).toBe(true);
		expect(decodeURIComponent(url.slice(url.indexOf(",") + 1))).toBe(
			'<svg width="20" height="10" xmlns="http://www.w3.org/2000/svg"><g/></svg>'
		);
	});
});

describe("why a drawing failed", () => {
	it("is the first line of the error, cut short", () => {
		expect(failureReason(new Error("Parse error on line 2:\n...a --> \n---^"))).toBe(
			"Parse error on line 2:"
		);
		expect(failureReason("x".repeat(400))).toHaveLength(160);
	});
});
