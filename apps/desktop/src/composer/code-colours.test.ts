// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { Editor } from "@tiptap/core";
import Document from "@tiptap/extension-document";
import Paragraph from "@tiptap/extension-paragraph";
import Text from "@tiptap/extension-text";
import { waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { type Language, type Lines, MAX_HIGHLIGHT } from "@/agent-view/highlight";

import { CodeColours } from "./code-colours";
import { COMPOSER_LOOK, MarkdownLook, markdownLook, type MarkdownLookOptions } from "./markdown";

const NOTE: MarkdownLookOptions = {
	maxLength: 1_000_000,
	byLine: true,
	tasks: true,
	links: true,
	images: true,
	html: true,
};

/** Every non-empty line as one token, in a colour that names its language. */
const linesFor = (code: string, lang: Language): Lines =>
	code.split("\n").map((line) => (line ? [{ content: line, color: `var(--${lang})` }] : []));

type Asked = { code: string; lang: Language; answer: (lines: Lines | null) => void };

/** A highlighter that answers only when a test says so. */
const fakeHighlighter = () => {
	const asked: Asked[] = [];
	const cache = new Map<string, Lines>();
	return {
		asked,
		cache,
		options: {
			highlight: (code: string, lang: Language) =>
				new Promise<Lines | null>((resolve) => asked.push({ code, lang, answer: resolve })),
			cached: (code: string, lang: Language) => cache.get(`${lang}:${code}`),
		},
		/** Answers the oldest question as a real highlighter would. */
		answer: async () => {
			const next = asked.shift();
			if (!next) throw new Error("Nothing was asked");
			next.answer(linesFor(next.code, next.lang));
			await Promise.resolve();
		},
	};
};

let editor: Editor | null = null;
afterEach(() => {
	editor?.destroy();
	editor = null;
});

const editorFor = (text: string, fake: ReturnType<typeof fakeHighlighter>, look = NOTE) => {
	editor = new Editor({
		element: document.createElement("div"),
		extensions: [
			Document,
			Paragraph,
			Text,
			MarkdownLook.configure(look),
			CodeColours.configure(fake.options),
		],
		content: {
			type: "doc",
			content: text
				.split("\n")
				.map((line) =>
					line === ""
						? { type: "paragraph" }
						: { type: "paragraph", content: [{ type: "text", text: line }] }
				),
		},
	});
	return editor;
};

/** The text each coloured span holds, with its colour. */
const coloured = (target: Editor) =>
	[...target.view.dom.querySelectorAll<HTMLElement>("span[style]")].map((span) => [
		span.textContent,
		span.style.color,
	]);

const textOf = (target: Editor) =>
	target.state.doc.textBetween(0, target.state.doc.content.size, "\n");

describe("markdownLook's fence code", () => {
	it("gives the code of a top-level fence that names a language", () => {
		const look = markdownLook("intro\n\n```ts\nconst a = 1;\nlet b;\n```", NOTE);
		expect(look.code).toEqual([{ lang: "ts", from: 17, text: "const a = 1;\nlet b;" }]);
	});

	it("leaves out indented code, fences without a language, and fences in a list", () => {
		expect(markdownLook("    indented\n    code", NOTE).code).toEqual([]);
		expect(markdownLook("```\nplain\n```", NOTE).code).toEqual([]);
		expect(markdownLook("- item\n\n  ```ts\n  let a;\n  ```", NOTE).code).toEqual([]);
		expect(markdownLook("```ts\n```", COMPOSER_LOOK).code).toEqual([]);
	});

	it("gives code with dollar signs as written", () => {
		expect(markdownLook("```ts\nconst s = `hi ${name}`;\n```", COMPOSER_LOOK).code).toEqual([
			{ lang: "ts", from: 8, text: "const s = `hi ${name}`;" },
		]);
	});

	it("keeps escapes and quote marks in a fence as code, with no markers", () => {
		for (const options of [NOTE, COMPOSER_LOOK]) {
			const look = markdownLook("```bash\n> a \\* b\n> > \\[c]\n```", options);
			expect(look.code).toEqual([{ lang: "bash", from: 10, text: "> a \\* b\n> > \\[c]" }]);
			const inside = look.markers.filter((m) => m.from >= 10 && m.to <= 29);
			expect(inside).toEqual([]);
			expect([...look.lines.values()].some((name) => name.includes("md-quote"))).toBe(false);
		}
	});

	it("gives an unclosed fence's code through the end", () => {
		expect(markdownLook("```rust\nfn main() {}", COMPOSER_LOOK).code).toEqual([
			{ lang: "rust", from: 10, text: "fn main() {}" },
		]);
	});
});

describe("CodeColours", () => {
	it("colours a fence's code once the highlighter answers, and leaves its fence lines alone", async () => {
		const fake = fakeHighlighter();
		const target = editorFor("Some text\n\n```ts\nconst a = 1;\n\nlet b;\n```", fake);
		expect(fake.asked.map(({ code, lang }) => [lang, code])).toEqual([
			["typescript", "const a = 1;\n\nlet b;"],
		]);
		expect(coloured(target)).toEqual([]);
		await fake.answer();
		await waitFor(() =>
			expect(coloured(target)).toEqual([
				["const a = 1;", "var(--typescript)"],
				["let b;", "var(--typescript)"],
			])
		);
		expect(textOf(target)).toBe("Some text\n\n```ts\nconst a = 1;\n\nlet b;\n```");
		// The fence's own lines keep the look they had.
		const fence = target.view.dom.querySelectorAll("p")[2]!;
		expect(fence.classList.contains("md-fence")).toBe(true);
		expect(fence.querySelector("span[style]")).toBeNull();
	});

	it("gives a diff block the diff grammar", () => {
		const fake = fakeHighlighter();
		editorFor("```diff\n-old\n+new\n```", fake);
		expect(fake.asked.map(({ lang }) => lang)).toEqual(["diff"]);
	});

	it("asks nothing for unknown languages, Mermaid, math or indented code", () => {
		const fake = fakeHighlighter();
		editorFor(
			"```klingon\nqapla'\n```\n\n```mermaid\ngraph TD\n```\n\n```math\nx^2\n```\n\n    indented",
			fake
		);
		expect(fake.asked).toEqual([]);
	});

	it("skips a fence over the chat's size cap", () => {
		const fake = fakeHighlighter();
		editorFor(`\`\`\`ts\n${"a".repeat(MAX_HIGHLIGHT + 1)}\n\`\`\``, fake);
		expect(fake.asked).toEqual([]);
	});

	it("colours at once from the cache, without asking", () => {
		const fake = fakeHighlighter();
		fake.cache.set("rust:fn main() {}", linesFor("fn main() {}", "rust"));
		const target = editorFor("```rust\nfn main() {}\n```", fake);
		expect(fake.asked).toEqual([]);
		expect(coloured(target)).toEqual([["fn main() {}", "var(--rust)"]]);
	});

	it("keeps the last colours while typing, asks again only for that fence, and drops stale replies", async () => {
		const fake = fakeHighlighter();
		const target = editorFor("```ts\nlet a;\n```\n\n```rust\nfn f() {}\n```", fake);
		await fake.answer();
		await fake.answer();
		await waitFor(() => expect(coloured(target)).toHaveLength(2));

		// Typing at the end of `let a;` in the first fence.
		target.commands.insertContentAt(14, "x");
		expect(coloured(target)).toEqual([
			["let a;", "var(--typescript)"],
			["fn f() {}", "var(--rust)"],
		]);
		expect(fake.asked.map(({ code }) => code)).toEqual(["let a;x"]);

		// Another key before the reply makes the reply stale.
		target.commands.insertContentAt(15, "y");
		await fake.answer();
		expect(coloured(target)).toEqual([
			["let a;", "var(--typescript)"],
			["fn f() {}", "var(--rust)"],
		]);
		await waitFor(() => expect(fake.asked.map(({ code }) => code)).toEqual(["let a;xy"]));
		await fake.answer();
		await waitFor(() =>
			expect(coloured(target)).toEqual([
				["let a;xy", "var(--typescript)"],
				["fn f() {}", "var(--rust)"],
			])
		);
		expect(fake.asked).toEqual([]);
	});

	it("drops a fence's colours when it stops naming a language it knows", async () => {
		const fake = fakeHighlighter();
		const target = editorFor("```ts\nlet a;\n```", fake);
		await fake.answer();
		await waitFor(() => expect(coloured(target)).toHaveLength(1));
		// `ts` becomes `tsq`.
		target.commands.insertContentAt(6, "q");
		expect(coloured(target)).toEqual([]);
		expect(fake.asked).toEqual([]);
	});

	it("keeps no colours outside a fence that a new fence line cuts short", async () => {
		const fake = fakeHighlighter();
		const target = editorFor("```ts\nlet a;\nlet b;\nlet c;\n```", fake);
		await fake.answer();
		await waitFor(() => expect(coloured(target)).toHaveLength(3));
		// `let b;` becomes a closing fence, and `let c;` is plain text after it.
		target.commands.insertContentAt({ from: 16, to: 22 }, "```");
		expect(textOf(target)).toBe("```ts\nlet a;\n```\nlet c;\n```");
		expect(coloured(target).map(([text]) => text)).not.toContain("let c;");
	});

	it("colours a fence typed into the composer", async () => {
		const fake = fakeHighlighter();
		const target = editorFor("Look:\n```py\nprint(1)\n```", fake, COMPOSER_LOOK);
		expect(fake.asked.map(({ lang }) => lang)).toEqual(["python"]);
		await fake.answer();
		await waitFor(() => expect(coloured(target)).toEqual([["print(1)", "var(--python)"]]));
	});
});
