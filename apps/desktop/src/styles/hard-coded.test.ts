// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Every color the app draws comes from a token, so both themes reach it.
// This reads the app's own files for colors written out in full: hex, the
// color functions, a named color in a style, and Tailwind's own palette.

import { describe, expect, it } from "vitest";

// Each of the app's own files as text, by its path in the app's folder.
const FILES: Readonly<Record<string, string>> = Object.fromEntries(
	Object.entries(
		import.meta.glob<string>(
			[
				"/src/**/*.{ts,tsx,css,html}",
				"!/src/**/*.test.*",
				"!/src/test/**",
				"/index.html",
				"/viewer.html",
			],
			{ query: "?raw", import: "default", eager: true }
		)
	).map(([file, text]) => [file.slice(1), text])
);

// The files that may write colors out, each with why.
const ALLOWED: ReadonlyMap<string, string> = new Map([
	// The themes' tokens themselves.
	["src/styles/globals.css", "the tokens"],
	// The viewer frame loads none of the app's styles, so it keeps its own pair.
	["viewer.html", "the viewer frame's own tokens"],
	// A scanner reads a code as black on white in either theme.
	["src/settings/phones-settings.tsx", "the pairing QR code"],
	// The count sits on the Windows taskbar, which follows the system's
	// theme, not the app's, so it keeps the system's red disc.
	["src/native/dock-badge.ts", "the taskbar overlay's count"],
	// The code theme's colours, each mapped to a token.
	["src/agent-view/highlight-worker.ts", "the code theme's colours"],
]);

const NAMED =
	"white|black|red|green|blue|yellow|orange|purple|pink|gray|grey|silver|maroon|navy|teal|olive|lime|aqua|fuchsia|cyan|magenta";
const PALETTE =
	"black|white|slate|gray|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose";
const UTILITY =
	"bg|text|border(?:-[trblxy])?|ring|ring-offset|fill|stroke|from|via|to|shadow|outline|decoration|divide|accent|caret|placeholder";

const COLORS: readonly RegExp[] = [
	/(?<![\w&/-])#(?:[\da-f]{8}|[\da-f]{6}|[\da-f]{3,4})(?![\w-])/gi,
	/(?<![a-z-])(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch)\(/gi,
	new RegExp(`(?<![\\w-])(?:${UTILITY})-(?:${PALETTE})(?:-\\d{2,3})?(?:/\\d+)?(?![\\w-])`, "g"),
	// A named color as a CSS value, a style's value or an SVG attribute.
	new RegExp(
		`(?:\\b(?:color|background|fill|stroke|border|outline)[\\w-]*\\s*[:=]\\s*["'{]?\\s*)(?:${NAMED})\\b`,
		"gi"
	),
];

/** The text with its comments taken out, keeping the lines where they were. */
const withoutComments = (text: string, file: string): string => {
	const blank = (found: string) => found.replace(/[^\n]/g, " ");
	if (file.endsWith(".html")) return text.replace(/<!--[\s\S]*?-->/g, blank);
	return text
		.replace(/\/\*[\s\S]*?\*\//g, blank)
		.replace(
			/(^|[^:"'`\\])\/\/.*$/gm,
			(found, before: string) => before + blank(found.slice(before.length))
		);
};

const SOURCES = Object.keys(FILES);

/** Each color written out in a file, as `line: text`. */
const colorsIn = (file: string): string[] => {
	const lines = withoutComments(FILES[file] ?? "", file).split("\n");
	return lines.flatMap((line, i) =>
		COLORS.flatMap((pattern) => [...line.matchAll(pattern)].map((m) => `${i + 1}: ${m[0]}`))
	);
};

describe("the app's colors", () => {
	it("finds the files to read", () => {
		expect(SOURCES).toContain("src/App.tsx");
		expect(SOURCES).toContain("src/main-area/panes.css");
		expect(SOURCES).toContain("index.html");
	});

	it("catches a color written out", () => {
		const found = (text: string) =>
			COLORS.some(
				(pattern) => [...withoutComments(text, "x.tsx").matchAll(pattern)].length > 0
			);
		for (const text of [
			'className="bg-[#141414]"',
			"shadow-[0_0_0_3px_rgb(70_146_246/0.18)]",
			"bg-black/50",
			"text-white",
			"border-zinc-700",
			'style={{ color: "red" }}',
			'fill="#000"',
			"background: oklch(0.5 0.1 200);",
		])
			expect(found(text), text).toBe(true);
		for (const text of [
			"// see #1234 for why",
			'const ask = "PR #42";',
			"bg-background text-foreground",
			"color-mix(in srgb, var(--primary) 18%, transparent)",
			'href="https://example.com/#abc"',
		])
			expect(found(text), text).toBe(false);
	});

	it.each(SOURCES.filter((file) => !ALLOWED.has(file)))("writes none out in %s", (file) => {
		expect(colorsIn(file)).toEqual([]);
	});

	it("allows only files that are there", () => {
		expect([...ALLOWED.keys()].filter((file) => !SOURCES.includes(file))).toEqual([]);
	});
});
