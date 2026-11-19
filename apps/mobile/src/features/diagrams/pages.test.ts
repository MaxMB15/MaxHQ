// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import { diagramTheme } from "@maxhq/shared/agent-view/diagram-theme";

import type { DrawJob } from "./drawings";
import { DIAGRAM_TOKENS, drawingOf, readAnswer, rendererPage, shownPage } from "./pages";

const job = (kind: DrawJob["kind"], size = 0): DrawJob => ({
	key: "k",
	kind,
	source: "x",
	size,
});

// The desktop's dark tokens, from the block that starts its stylesheet.
const desktopDark = (): Map<string, string> => {
	const css = readFileSync(
		new URL("../../../../desktop/src/styles/globals.css", import.meta.url),
		"utf8"
	);
	const start = css.indexOf(':root[data-theme="dark"] {');
	const block = css.slice(start, css.indexOf("\n}", start));
	return new Map(
		[...block.matchAll(/(--[\w-]+):\s*([^;]+);/g)].map(([, name, value]) => [
			name!,
			value!.trim(),
		])
	);
};

describe("a diagram's colours", () => {
	it("are the desktop's dark ones, for every token the theme asks for", () => {
		const asked = new Set<string>();
		diagramTheme((name) => {
			asked.add(name);
			return name;
		}, "dark");
		const desktop = desktopDark();
		expect(asked.size).toBeGreaterThan(10);
		for (const name of asked) {
			expect(DIAGRAM_TOKENS[name], name).toBeDefined();
			expect(DIAGRAM_TOKENS[name], name).toBe(desktop.get(name));
		}
	});
});

describe("a drawing page", () => {
	const libraries = new Map([
		["mermaid.js", "/* mermaid </script> */"],
		["katex.js", "/* katex */"],
	]);

	it("holds its library and the desktop's settings, and nothing that closes its script", () => {
		const page = rendererPage("diagram", libraries);
		expect(page).toContain("/* mermaid <\\/script> */");
		expect(page).not.toContain("katex */");
		expect(page).toContain('"securityLevel":"strict"');
		expect(page).toContain("Content-Security-Policy");
		expect(page).toContain("connect-src 'none'");
		expect(page).toContain("window.maxhqDraw");
		expect(page).not.toContain("maxhq:lib/");
	});

	it("draws formulas as MathML", () => {
		const page = rendererPage("math", libraries);
		expect(page).toContain("/* katex */");
		expect(page).not.toContain("mermaid");
		expect(page).toContain('"output":"mathml"');
		expect(page).toContain('"trust":false');
	});
});

describe("a drawing page's answer", () => {
	it("is read when it has its parts", () => {
		expect(readAnswer('{"type":"ready"}')).toEqual({ type: "ready" });
		expect(
			readAnswer(
				JSON.stringify({
					type: "diagram",
					key: "k",
					maxWidth: "200px",
					width: "100%",
					viewBox: "0 0 400 100",
					svg: "<svg></svg>",
				})
			)
		).toMatchObject({ type: "diagram", key: "k" });
		expect(
			readAnswer('{"type":"math","key":"k","markup":"<math></math>","width":20,"height":9}')
		).toMatchObject({ type: "math", width: 20 });
		expect(readAnswer('{"type":"empty","key":"k"}')).toEqual({ type: "empty", key: "k" });
		expect(readAnswer('{"type":"failed","key":"k","reason":"no"}')).toEqual({
			type: "failed",
			key: "k",
			reason: "no",
		});
	});

	it("is nothing when it's anything else", () => {
		for (const data of [
			"not json",
			"null",
			'{"type":"maxhq-visual:height","height":20}',
			'{"type":"empty"}',
			'{"type":"diagram","key":"k","maxWidth":"","width":null,"viewBox":null}',
			'{"type":"math","key":"k","markup":"","width":-1,"height":9}',
			'{"type":"math","key":"k","markup":"","width":"20","height":9}',
			'{"type":"failed","key":"k"}',
			'{"type":"other","key":"k"}',
		])
			expect(readAnswer(data), data).toBeNull();
	});
});

describe("the drawing an answer makes", () => {
	it("shows a diagram as a picture at its size", () => {
		const drawing = drawingOf(
			{
				type: "diagram",
				key: "k",
				maxWidth: "200px",
				width: "100%",
				viewBox: "0 0 400 100",
				svg: '<svg xmlns="http://www.w3.org/2000/svg"></svg>',
			},
			job("diagram")
		);
		expect(drawing).toMatchObject({ state: "drawn", width: 200, height: 50 });
		expect(drawing.state === "drawn" && drawing.body).toMatch(
			/^<img src="data:image\/svg\+xml;charset=utf-8,[^"]+" alt="">$/
		);
	});

	it("shows a formula at its font size", () => {
		const drawing = drawingOf(
			{ type: "math", key: "k", markup: "<math></math>", width: 30, height: 12 },
			job("math", 17)
		);
		expect(drawing).toEqual({
			state: "drawn",
			body: '<div class="math" style="font-size:17px"><math></math></div>',
			width: 30,
			height: 12,
		});
	});

	it("says why it couldn't", () => {
		const diagram = job("diagram");
		expect(
			drawingOf(
				{ type: "diagram", key: "k", maxWidth: "", width: null, viewBox: null, svg: "" },
				diagram
			)
		).toEqual({ state: "failed", reason: "The drawing has no size." });
		expect(
			drawingOf({ type: "math", key: "k", markup: "", width: 0, height: 0 }, job("math", 17))
		).toEqual({ state: "failed", reason: "The drawing has no size." });
		expect(drawingOf({ type: "empty", key: "k" }, diagram)).toEqual({
			state: "failed",
			reason: "The drawing came out empty.",
		});
		expect(
			drawingOf({ type: "failed", key: "k", reason: "Parse error on line 2:\n---^" }, diagram)
		).toEqual({ state: "failed", reason: "Parse error on line 2:" });
	});
});

describe("a shown page", () => {
	it("holds the drawing and runs nothing", () => {
		const page = shownPage('<img src="data:," alt="">');
		expect(page).toContain("default-src 'none'; img-src data:; style-src 'unsafe-inline'");
		expect(page).not.toContain("script");
		expect(page.endsWith('<img src="data:," alt="">')).toBe(true);
	});
});
