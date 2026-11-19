// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import {
	codeColors,
	createHighlighter,
	isBold,
	isItalic,
	type Language,
	languageOf,
	type Lines,
	MAX_HIGHLIGHT,
} from "./highlight";

const linesOf = (code: string): Lines => code.split("\n").map((line) => [{ content: line }]);

// A highlighter whose runs are counted and finished by hand.
const counted = () => {
	const runs: { code: string; lang: Language; done: (lines: Lines | null) => void }[] = [];
	const highlighter = createHighlighter(
		(code, lang) => new Promise((done) => runs.push({ code, lang, done }))
	);
	return { highlighter, runs };
};

describe("languageOf", () => {
	it("knows languages by name, alias and extension, in any case", () => {
		expect(languageOf("TypeScript")).toBe("typescript");
		expect(languageOf(" rs ")).toBe("rust");
		expect(languageOf("yml")).toBe("yaml");
		expect(languageOf("bash")).toBe("shellscript");
	});

	it("gives null for nothing and for languages it can't do", () => {
		expect(languageOf(undefined)).toBeNull();
		expect(languageOf("")).toBeNull();
		expect(languageOf("brainfuck")).toBeNull();
	});
});

describe("codeColors", () => {
	it("turns each theme colour into a client's own", () => {
		const colors = codeColors((name) => `--${name}`);
		expect(colors["#8b949e"]).toBe("--comment");
		expect(colors["#e6edf3"]).toBe("--text");
		expect(colors["#f0f6fc"]).toBe("--text");
	});
});

describe("font styles", () => {
	it("reads Shiki's italic and bold bits", () => {
		expect(isItalic({ content: "", fontStyle: 1 })).toBe(true);
		expect(isBold({ content: "", fontStyle: 1 })).toBe(false);
		expect(isBold({ content: "", fontStyle: 3 })).toBe(true);
		expect(isItalic({ content: "" })).toBe(false);
	});
});

describe("createHighlighter", () => {
	it("runs once for the same code asked twice at once, and caches it", async () => {
		const { highlighter, runs } = counted();
		const first = highlighter.highlight("a\nb", "rust");
		const second = highlighter.highlight("a\nb", "rust");
		expect(runs).toHaveLength(1);
		runs[0]!.done(linesOf("a\nb"));
		expect(await first).toEqual(linesOf("a\nb"));
		expect(await second).toBe(await first);
		expect(highlighter.cachedLines("a\nb", "rust")).toBe(await first);
		expect(await highlighter.highlight("a\nb", "rust")).toBe(await first);
		expect(runs).toHaveLength(1);
	});

	it("keeps the same code in two languages apart", async () => {
		const { highlighter, runs } = counted();
		void highlighter.highlight("x", "rust");
		void highlighter.highlight("x", "go");
		expect(runs.map((run) => run.lang)).toEqual(["rust", "go"]);
	});

	it("leaves code longer than the most plain without running", async () => {
		const { highlighter, runs } = counted();
		expect(await highlighter.highlight("x".repeat(MAX_HIGHLIGHT + 1), "go")).toBeNull();
		expect(await highlighter.highlight("abcd", "go", 3)).toBeNull();
		expect(runs).toHaveLength(0);
	});

	it("gives null when a run fails, and tries again on the next ask", async () => {
		let calls = 0;
		const highlighter = createHighlighter(async () => {
			calls += 1;
			if (calls === 1) throw new Error("no grammar");
			return linesOf("x");
		});
		expect(await highlighter.highlight("x", "go")).toBeNull();
		expect(highlighter.cachedLines("x", "go")).toBeUndefined();
		expect(await highlighter.highlight("x", "go")).toEqual(linesOf("x"));
	});

	it("forgets the least recently used code past 300 blocks", async () => {
		const highlighter = createHighlighter(async (code) => linesOf(code));
		await highlighter.highlight("first", "go");
		await highlighter.highlight("second", "go");
		// Using the first again makes the second the oldest.
		expect(highlighter.cachedLines("first", "go")).toBeDefined();
		for (let i = 0; i < 299; i++) await highlighter.highlight(`code ${i}`, "go");
		expect(highlighter.cachedLines("first", "go")).toBeDefined();
		expect(highlighter.cachedLines("second", "go")).toBeUndefined();
	});
});
