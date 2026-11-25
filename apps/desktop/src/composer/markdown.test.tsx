// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { Editor } from "@tiptap/core";
import Document from "@tiptap/extension-document";
import Paragraph from "@tiptap/extension-paragraph";
import Text from "@tiptap/extension-text";
import { act, fireEvent, waitFor } from "@testing-library/react";
import { beforeAll, describe, expect, it, vi } from "vitest";

import { OBJECT_MARK } from "@maxhq/shared/pills/draft";

import {
	blocksParsed,
	COMPOSER_LOOK,
	MarkdownLook,
	markdownLook,
	type MarkdownLookOptions,
	MAX_MARKDOWN_LENGTH,
	NO_DEFINITIONS,
} from "./markdown";
import {
	AGENT,
	editorOf,
	findTextbox,
	installBrowserFakes,
	renderComposer,
	textIn,
	type,
} from "./testing";

vi.mock("@/agent-view/diagram-render", () => {
	const url = "data:image/svg+xml,drawn";
	const drawn = { state: "drawn", url, width: 200, height: 100, whole: () => url };
	return { drawnDiagram: () => drawn, drawDiagram: () => Promise.resolve(drawn) };
});

beforeAll(installBrowserFakes);

const NOTE: MarkdownLookOptions = {
	maxLength: 1_000_000,
	byLine: true,
	tasks: true,
	links: true,
	images: true,
	html: true,
};

/** The line classes of `text`, one entry per line that has any. */
const linesOf = (text: string, options = COMPOSER_LOOK) =>
	Object.fromEntries(markdownLook(text, options).lines);

/** The text each run of `text` covers, by its class. A document range on one line reads straight from it. */
const runsOf = (text: string, options = COMPOSER_LOOK) =>
	markdownLook(text, options).runs.map((run) => [
		run.className,
		text.slice(run.from - 1, run.to - 1),
	]);

/** An editor with a note's look, one paragraph per line of `text`, with the focus unless `blurred`. */
const noteEditor = (text: string, blurred = false) => {
	const element = document.createElement("div");
	document.body.append(element);
	const editor = new Editor({
		element,
		extensions: [Document, Paragraph, Text, MarkdownLook.configure(NOTE)],
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
	if (!blurred) editor.view.focus();
	return editor;
};

/** The document position of an offset in a text, with each line break two positions. */
const posAt = (text: string, offset: number) => offset + text.slice(0, offset).split("\n").length;

describe("markdownLook", () => {
	it("marks bold, italic and struck text, with their markers", () => {
		expect(runsOf("**a** *b* _c_ ~~d~~")).toEqual([
			["md-strong", "a"],
			["md-em", "b"],
			["md-em", "c"],
			["md-strike", "d"],
		]);
		const whole = { from: 1, to: 6 };
		expect(markdownLook("**a**").markers).toEqual([
			{ from: 1, to: 3, within: whole },
			{ from: 4, to: 6, within: whole },
		]);
	});

	it("marks inline code, however many backticks it takes", () => {
		expect(runsOf("`a` and ```long snippet```")).toEqual([
			["md-code", "a"],
			["md-code", "long snippet"],
		]);
		expect(markdownLook("```long snippet```").fences).toEqual([]);
	});

	it("leaves text alone that only looks like markup", () => {
		expect(markdownLook("2 * 3 * 4 and snake_case_name and `one tick")).toEqual({
			runs: [],
			markers: [],
			lines: new Map(),
			fences: [],
			code: [],
			formulas: [],
			previews: [],
			tasks: [],
			links: [],
			footnotes: [],
			defines: NO_DEFINITIONS,
		});
	});

	it("hides a link's address, and has nothing to hide on a bare one", () => {
		expect(runsOf("[site](https://x.co)")).toEqual([["md-link", "site"]]);
		const look = markdownLook("[site](https://x.co)");
		expect(look.markers.map(({ from, to }) => [from, to])).toEqual([
			[1, 2],
			[6, 21],
		]);
		expect(runsOf("see https://x.co")).toEqual([["md-link", "https://x.co"]]);
		expect(markdownLook("see https://x.co").markers).toEqual([]);
	});

	it("counts each line break as two document positions, and a pill as one", () => {
		const { runs } = markdownLook("one\ntwo **b**");
		expect(runs).toEqual([{ from: 12, to: 13, className: "md-strong" }]);
		const pilled = markdownLook(`${OBJECT_MARK} **b**`);
		expect(pilled.runs).toEqual([{ from: 5, to: 6, className: "md-strong" }]);
	});

	it("draws a closed fence's lines as one block", () => {
		const look = markdownLook("before\n```ts\nconst a = 1;\n```\nafter");
		expect(Object.fromEntries(look.lines)).toEqual({
			1: "md-code-line md-code-first md-fence",
			2: "md-code-line",
			3: "md-code-line md-code-last md-fence",
		});
		expect(look.fences).toEqual([{ first: 1, last: 2 }]);
		expect(look.runs).toEqual([]);
	});

	it("runs an unclosed fence on to the end of the text, blank lines included", () => {
		expect(markdownLook("```").fences).toEqual([{ first: 0, last: 0 }]);
		expect(markdownLook("```\nfoo\n").fences).toEqual([{ first: 0, last: 2 }]);
		expect(markdownLook("```\nfoo\n\n").fences).toEqual([{ first: 0, last: 3 }]);
		expect(linesOf("```\n")).toEqual({
			0: "md-code-line md-code-first md-fence",
			1: "md-code-line md-code-last",
		});
	});

	it("needs a closing fence of the same kind and at least the same length", () => {
		expect(markdownLook("~~~\n```").fences).toEqual([{ first: 0, last: 1 }]);
		expect(markdownLook("````\n```").fences).toEqual([{ first: 0, last: 1 }]);
		expect(markdownLook("```\n````").fences).toEqual([{ first: 0, last: 0 }]);
	});

	it("doesn't read Markdown inside code", () => {
		expect(markdownLook("```\n**a**\n```").runs).toEqual([]);
		expect(markdownLook("`**a**`").runs).toEqual([{ from: 2, to: 7, className: "md-code" }]);
	});

	it("draws an indented code block without Enter's fence rule", () => {
		const look = markdownLook("    code");
		expect(Object.fromEntries(look.lines)).toEqual({
			0: "md-code-line md-code-first md-code-last",
		});
		expect(look.fences).toEqual([]);
	});

	it("sizes a heading by its level, and hides its marker", () => {
		expect(linesOf("# a")).toEqual({ 0: "md-h1" });
		expect(linesOf("###### a")).toEqual({ 0: "md-h4" });
		const whole = { from: 1, to: 4 };
		expect(markdownLook("# a").markers).toEqual([{ from: 1, to: 3, within: whole }]);
	});

	it("keeps a heading's marker showing while nothing follows it", () => {
		expect(markdownLook("#").markers).toEqual([{ from: 1, to: 2, within: null }]);
	});

	it("sizes an underlined heading, and always shows its underline", () => {
		expect(linesOf("a\n===")).toEqual({ 0: "md-h1" });
		expect(markdownLook("a\n===").markers).toEqual([{ from: 4, to: 7, within: null }]);
		expect(linesOf("a\nb\n---")).toEqual({ 0: "md-h2", 1: "md-h2" });
		expect(runsOf("**a**\n---")).toEqual([["md-strong", "a"]]);
	});

	it("draws a rule as a line once the selection is off it", () => {
		expect(linesOf("---")).toEqual({ 0: "md-rule" });
		expect(markdownLook("---").markers).toEqual([
			{ from: 1, to: 4, within: { from: 1, to: 4 }, fold: 0 },
		]);
		expect(markdownLook("- ***").markers).toContainEqual({ from: 3, to: 6, within: null });
		expect(linesOf("- ***")).toEqual({});
	});

	it("leaves raw HTML as text, since a sent message shows it so", () => {
		for (const text of [
			"a <mark>b</mark> <kbd>K</kbd> c<br>d <!-- e -->",
			"<details>\n<summary>S</summary>\nx\n</details>",
		]) {
			const look = markdownLook(text);
			expect(look.runs).toEqual([]);
			expect(look.markers).toEqual([]);
			expect(look.previews).toEqual([]);
		}
	});

	it("shows a quote's marks with the caret on their line, and always shows list markers", () => {
		expect(linesOf("> a\n> b")).toEqual({ 0: "md-quote", 1: "md-quote" });
		expect(markdownLook("> a\n> b").markers).toEqual([
			{ from: 1, to: 3, within: { from: 1, to: 4 } },
			{ from: 6, to: 8, within: { from: 6, to: 9 } },
		]);
		expect(markdownLook("- a").markers).toEqual([{ from: 1, to: 3, within: null }]);
		expect(markdownLook("1. a").markers).toEqual([{ from: 1, to: 4, within: null }]);
	});

	it("reads markup inside a heading, a quote and a list", () => {
		expect(runsOf("# **a**")).toEqual([["md-strong", "a"]]);
		expect(runsOf("> **a**")).toEqual([["md-strong", "a"]]);
		expect(runsOf("- **a**")).toEqual([["md-strong", "a"]]);
	});

	it("leaves tables as they're typed, since their pipes line up", () => {
		const look = markdownLook("| **a** |\n| --- |\n| b |");
		expect(look.runs).toEqual([]);
		expect(look.markers).toEqual([]);
	});

	it("draws a closed Mermaid block whole, with the caret going to its first line", () => {
		const text = "before\n```mermaid\ngraph TD\n  A --> B\n```\nafter";
		expect(markdownLook(text).previews).toEqual([
			{
				kind: "mermaid",
				source: "graph TD\n  A --> B",
				first: 1,
				last: 4,
				within: { from: 9, to: 45 },
				caret: 21,
			},
		]);
	});

	it("leaves code alone that isn't a finished Mermaid block", () => {
		for (const text of [
			"```ts\nconst a = 1\n```",
			"```mermaid\ngraph TD",
			"```mermaid\n\n```",
			"> ```mermaid\n> graph TD\n> ```",
			`\`\`\`mermaid\ngraph TD\n  A --> ${OBJECT_MARK}\n\`\`\``,
		])
			expect(markdownLook(text).previews).toEqual([]);
	});

	it("reads a formula block as a code block, and draws it once it's closed", () => {
		const open = markdownLook("$$\nx^2");
		expect(open.previews).toEqual([]);
		expect(open.fences).toEqual([{ first: 0, last: 1 }]);
		const closed = markdownLook("$$\nx^2\n$$");
		expect(closed.previews).toMatchObject([{ kind: "math", source: "x^2", first: 0, last: 2 }]);
		expect(closed.fences).toEqual([{ first: 0, last: 1 }]);
		expect(closed.lines.get(0)).toContain("md-code-line");
	});

	it("draws a table whole, and leaves one with a picture or a pill as text", () => {
		const table = "| a |\n| - |\n| 1 |";
		expect(markdownLook(`${table}\n\nafter`).previews).toMatchObject([
			{ kind: "table", source: table, first: 0, last: 2, caret: 1 },
		]);
		expect(markdownLook("| a |\n| - |\n| ![x](y.png) |").previews).toEqual([]);
		expect(markdownLook(`| a |\n| - |\n| ${OBJECT_MARK} |`).previews).toEqual([]);
	});

	it("finds a formula in a sentence, and leaves dollars that are money", () => {
		expect(markdownLook("so $x^2$ holds").formulas).toEqual([
			{ from: 4, to: 9, source: "x^2" },
		]);
		expect(markdownLook("from $5 to $10").formulas).toEqual([]);
		expect(markdownLook("US$5 and $HOME").formulas).toEqual([]);
	});

	it("leaves a formula across two lines as text, since their break can't hide", () => {
		expect(markdownLook("$a\nb$").formulas).toEqual([]);
		expect(markdownLook(`$a ${OBJECT_MARK}$`).formulas).toEqual([]);
	});

	it("stops past the longest text it keeps up with", () => {
		expect(markdownLook(`**a** ${"x".repeat(MAX_MARKDOWN_LENGTH)}`).runs).toEqual([]);
		expect(markdownLook(`**a** ${"x".repeat(MAX_MARKDOWN_LENGTH - 6)}`).runs).toHaveLength(1);
	});
});

describe("markdownLook in a note", () => {
	it("shows a span's markers while the caret is on its line", () => {
		const line = { from: 1, to: 12 };
		expect(markdownLook("say **a** b", NOTE).markers).toEqual([
			{ from: 5, to: 7, within: line },
			{ from: 8, to: 10, within: line },
		]);
	});

	it("folds a fence's lines while the caret is out of its block", () => {
		const look = markdownLook("```js\nx\n```", NOTE);
		const block = { from: 1, to: 14 };
		expect(look.markers).toEqual([
			{ from: 1, to: 6, within: block, fold: 0 },
			{ from: 11, to: 14, within: block, fold: 2 },
		]);
		expect(markdownLook("```js\nx\n```").markers).toEqual([]);
	});

	it("finds a task's box, hides its dash with it, and strikes a done one's words", () => {
		const look = markdownLook("- [ ] one\n- [x] **two**", NOTE);
		expect(look.tasks).toEqual([
			{ from: 3, to: 6, checked: false, within: { from: 1, to: 10 } },
			{ from: 14, to: 17, checked: true, within: { from: 12, to: 25 } },
		]);
		expect(look.markers).toContainEqual({ from: 1, to: 3, within: { from: 1, to: 10 } });
		expect(look.markers).toContainEqual({ from: 12, to: 14, within: { from: 12, to: 25 } });
		expect(look.lines.get(1)).toBe("md-task-done");
		expect(look.runs).toContainEqual({ from: 18, to: 25, className: "md-task-done-text" });
		expect(markdownLook("- [ ] one").tasks).toEqual([]);
	});

	it("finds links and pictures on their own lines, but not ones on a device", () => {
		expect(markdownLook("[a](https://x.co)", NOTE).links).toEqual([
			{ from: 1, to: 18, url: "https://x.co" },
		]);
		expect(
			markdownLook("![a](https://x.co/a.png)", NOTE).previews.map((each) => each.kind)
		).toEqual(["image"]);
		expect(markdownLook("![a](/home/a.png)", NOTE).previews).toEqual([]);
		expect(markdownLook("![a](https://x.co/a.png)", COMPOSER_LOOK).previews).toEqual([]);
	});

	it("folds an underlined heading's underline while the caret is off the heading", () => {
		expect(linesOf("a\n===", NOTE)).toEqual({ 0: "md-h1" });
		expect(markdownLook("a\n===", NOTE).markers).toEqual([
			{ from: 4, to: 7, within: { from: 1, to: 7 }, fold: 1 },
		]);
	});

	it("folds a rule's line while the caret is off it", () => {
		expect(markdownLook("a\n\n***", NOTE).markers).toEqual([
			{ from: 6, to: 9, within: { from: 6, to: 9 }, fold: 2 },
		]);
	});

	it("hides a pair of tags a reply draws, and gives what they hold its look", () => {
		const text = "a <mark>b **c**</mark> <kbd>K</kbd>";
		expect(runsOf(text, NOTE)).toEqual([
			["md-tag-mark", "b **c**"],
			["md-tag-kbd", "K"],
			["md-strong", "c"],
		]);
		const line = { from: 1, to: 36 };
		expect(markdownLook(text, NOTE).markers).toEqual(
			expect.arrayContaining([
				{ from: 3, to: 9, within: line },
				{ from: 16, to: 23, within: line },
				{ from: 24, to: 29, within: line },
				{ from: 30, to: 36, within: line },
			])
		);
		expect(runsOf("<sub>2</sub> and <SUP>3</SUP>", NOTE)).toEqual([
			["md-tag-sub", "2"],
			["md-tag-sup", "3"],
		]);
	});

	it("leaves a tag with no pair, or one a reply doesn't draw, as text", () => {
		for (const text of ["a <mark>b", "a </kbd> b", "<foo>x</foo>", "<mark>a</kbd>"]) {
			const look = markdownLook(text, NOTE);
			expect(look.runs).toEqual([]);
			expect(look.markers).toEqual([]);
		}
	});

	it("breaks the line at a hidden <br>, and dims a comment without hiding it", () => {
		expect(markdownLook("a<br>b", NOTE).markers).toEqual([
			{ from: 2, to: 6, within: { from: 1, to: 7 }, lineBreak: true },
		]);
		expect(runsOf("a <!-- b --> c", NOTE)).toEqual([["md-comment", "<!-- b -->"]]);
		expect(markdownLook("a <!-- b --> c", NOTE).markers).toEqual([]);
	});

	it("draws an HTML block whole, from its opening tag through its closing one", () => {
		const list = "<dl>\n<dt>Term</dt>\n<dd>Its definition.</dd>\n</dl>";
		expect(markdownLook(list, NOTE).previews).toEqual([
			{
				kind: "html",
				source: list,
				first: 0,
				last: 3,
				within: { from: 1, to: 53 },
				caret: 1,
			},
		]);
		const details = "<details>\n<summary>S</summary>\n\nHidden **x**\n\n</details>";
		const look = markdownLook(`${details}\n\nafter`, NOTE);
		expect(look.previews).toMatchObject([{ kind: "html", source: details, first: 0, last: 5 }]);
		expect(look.runs).toEqual([{ from: 45, to: 46, className: "md-strong" }]);
	});

	it("leaves an unclosed HTML block, a comment, and one with a picture as text", () => {
		for (const text of [
			"<details>\n<summary>S</summary>\n\nHidden",
			"<details>\n<details>\n</details>",
			"<!-- a -->",
			"<div>\n<img src=a.png>\n</div>",
			"<details>\n\n![a](https://x.co/a.png)\n\n</details>",
			`<div>\n${OBJECT_MARK}\n</div>`,
		])
			expect(
				markdownLook(text, NOTE).previews.filter((each) => each.kind === "html")
			).toEqual([]);
	});
});

describe("callouts, nested quotes, footnotes, references and escapes", () => {
	const hiddenIn = (editor: Editor) =>
		[...editor.view.dom.querySelectorAll(".md-hidden")].map((each) => each.textContent);

	it("draws a callout whole, as the message will, in its colour", () => {
		const text = "> [!TIP] Try this\n> It helps.";
		const look = markdownLook(text);
		expect(look.previews).toMatchObject([{ kind: "callout", source: text, first: 0, last: 1 }]);
		expect(look.previews[0]).not.toHaveProperty("reply");
		expect(linesOf(text)).toEqual({
			0: "md-callout md-tone-green md-quote",
			1: "md-callout md-tone-green md-quote",
		});
		expect(markdownLook(text, NOTE).previews).toMatchObject([{ kind: "callout", reply: true }]);
		expect(markdownLook("> [!NOPE]\n> a").previews).toEqual([]);
	});

	it("draws a callout with a pill or a picture as a quote, its kind dimmed", () => {
		for (const body of [OBJECT_MARK, "![a](https://x.co/a.png)"]) {
			const look = markdownLook(`> [!WARNING]\n> ${body}`, NOTE);
			expect(look.previews.filter((each) => each.kind === "callout")).toEqual([]);
			expect(look.markers).toContainEqual({ from: 3, to: 13, within: null });
			expect(look.lines.get(0)).toBe("md-callout md-tone-amber md-quote");
		}
	});

	it("shows a callout's source with the caret in it, and draws it again once the caret leaves", async () => {
		const text = "> [!NOTE]\n> Useful.\n\nafter";
		const editor = noteEditor(text);
		editor.commands.setTextSelection(posAt(text, text.length));
		expect(editor.view.dom.querySelectorAll(".md-preview")).toHaveLength(1);
		await waitFor(() =>
			expect(editor.view.dom.querySelector(".md-preview")?.textContent?.trim()).toBe(
				"Note\nUseful."
			)
		);
		editor.commands.setTextSelection(posAt(text, 3));
		expect(editor.view.dom.querySelector(".md-preview")).toBeNull();
		editor.destroy();
	});

	it("draws a bar for each level of a quote, and hides each mark off its line", () => {
		const text = "> a\n>\n> > b";
		expect(linesOf(text)).toEqual({ 0: "md-quote", 1: "md-quote", 2: "md-quote md-quote-2" });
		const third = { from: 9, to: 14 };
		expect(markdownLook(text).markers).toEqual([
			{ from: 1, to: 3, within: { from: 1, to: 4 } },
			{ from: 6, to: 7, within: { from: 6, to: 7 } },
			{ from: 9, to: 11, within: third },
			{ from: 11, to: 13, within: third },
		]);
		const editor = noteEditor(`${text}\n\nafter`);
		editor.commands.setTextSelection(posAt(text, text.length + 3));
		expect(hiddenIn(editor)).toEqual(["> ", ">", "> ", "> "]);
		editor.commands.setTextSelection(posAt(text, text.length));
		expect(hiddenIn(editor)).toEqual(["> ", ">"]);
		editor.destroy();
	});

	it("numbers footnotes as a reply does, by their first call, across blocks", () => {
		const text =
			"A claim.[^1] A named one.[^named] Again.[^1]\n\n[^named]: Named, see [^late].\n[^1]: First.\n[^late]: Late.";
		const editor = noteEditor(text);
		editor.commands.setTextSelection(posAt(text, text.length));
		const numbers = () =>
			[...editor.view.dom.querySelectorAll(".md-footnote")].map((each) => each.textContent);
		expect(numbers()).toEqual(["1", "2", "1", "3"]);
		expect(hiddenIn(editor)).toEqual(["[^1]", "[^named]", "[^1]", "[^late]"]);
		// A definition's label stays, dimmed.
		const shown = [...editor.view.dom.querySelectorAll(".md-marker:not(.md-hidden)")].map(
			(each) => each.textContent
		);
		expect(shown).toEqual(expect.arrayContaining(["[^named]:", "[^1]:", "[^late]:"]));
		// A call in a footnote's own text comes after the rest.
		editor.commands.setTextSelection(1);
		expect(numbers()).toEqual(["3"]);
		editor.destroy();
	});

	it("shows a footnote's call with the caret on its line, and only numbers ones it can find", () => {
		const text = "A.[^1] B.[^none]\n\n[^1]: One.";
		const editor = noteEditor(text);
		editor.commands.setTextSelection(posAt(text, 1));
		expect(editor.view.dom.querySelector(".md-footnote")).toBeNull();
		editor.commands.setTextSelection(posAt(text, text.length));
		expect(
			[...editor.view.dom.querySelectorAll(".md-footnote")].map((each) => each.textContent)
		).toEqual(["1"]);
		expect(editor.getText({ blockSeparator: "\n" })).toBe(text);
		editor.destroy();
	});

	it("reads each kind of reference link, and opens its address on a Control-click off a Mac", () => {
		const open = vi.spyOn(window, "open").mockReturnValue(null);
		const text = "[full][spec], [spec][] and [spec]\n\n[spec]: https://spec.example.com";
		const editor = noteEditor(text);
		editor.commands.setTextSelection(posAt(text, text.length));
		const links = [...editor.view.dom.querySelectorAll(".md-link")].map(
			(each) => each.textContent
		);
		expect(links).toEqual(["full", "spec", "spec"]);
		expect(hiddenIn(editor)).toEqual(["[", "][spec]", "[", "][]", "[", "]"]);
		const definition = editor.view.dom.querySelectorAll("p")[2]!.querySelector(".md-marker");
		expect(definition?.textContent).toBe("[spec]: https://spec.example.com");
		editor.view.posAtCoords = () => ({ pos: posAt(text, 2), inside: -1 });
		// The Windows key, Command's place, isn't Control.
		fireEvent.mouseDown(editor.view.dom, { metaKey: true, button: 0 });
		expect(open).not.toHaveBeenCalled();
		fireEvent.mouseDown(editor.view.dom, { ctrlKey: true, button: 0 });
		expect(open).toHaveBeenCalledWith("https://spec.example.com", "_blank", "noreferrer");
		open.mockRestore();
		editor.destroy();
	});

	it("leaves a reference with no definition as text", () => {
		expect(runsOf("[full][nowhere] and [nowhere]")).toEqual([]);
		expect(runsOf("[a][x]\n\n[x]: https://x.co")).toEqual([["md-link", "a"]]);
	});

	it("hides an escape's backslash and a hard break's, off the caret", () => {
		const text = "a \\*b\\* c\\\nd";
		expect(runsOf(text)).toEqual([]);
		expect(markdownLook(text).markers).toEqual([
			{ from: 3, to: 4, within: { from: 3, to: 5 } },
			{ from: 6, to: 7, within: { from: 6, to: 8 } },
			{ from: 10, to: 11, within: { from: 10, to: 11 } },
		]);
		expect(markdownLook("a\\b and C:\\path").markers).toEqual([]);
		expect(markdownLook("a\nb \\# c").markers).toEqual([
			{ from: 6, to: 7, within: { from: 6, to: 8 } },
		]);
		const editor = noteEditor(`${text}\n\nafter`);
		editor.commands.setTextSelection(posAt(text, text.length + 3));
		expect(hiddenIn(editor)).toEqual(["\\", "\\", "\\"]);
		// A hard break's backslash ends its line, so it shows with the caret there.
		editor.commands.setTextSelection(1);
		expect(hiddenIn(editor)).toEqual([]);
		editor.destroy();
	});

	it("reads definitions in other blocks, and reads every block again only when one comes or goes", () => {
		// A Mac opens a link on a Command-click.
		vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
			"Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)"
		);
		const open = vi.spyOn(window, "open").mockReturnValue(null);
		const text =
			"[spec] and a note.[^1]\n\nplain\n\n[spec]: https://one.example.com\n\n[^1]: One.";
		const editor = noteEditor(text);
		const links = () => editor.view.dom.querySelector("p")!.querySelectorAll(".md-link").length;
		const notes = () => editor.view.dom.querySelectorAll(".md-footnote").length;
		editor.commands.setTextSelection(posAt(text, text.indexOf("plain")));
		expect(links()).toBe(1);
		expect(notes()).toBe(1);

		// A plain edit reads its own block.
		editor.commands.insertContent("x");
		expect(blocksParsed(editor.state)).toBe(1);

		// A new address reads only its own block too, and the link follows it.
		const url = posAt(editor.getText({ blockSeparator: "\n" }), text.indexOf("one.") + 1);
		editor.commands.setTextSelection(url);
		editor.commands.insertContent("x");
		expect(blocksParsed(editor.state)).toBe(1);
		editor.view.posAtCoords = () => ({ pos: 2, inside: -1 });
		fireEvent.mouseDown(editor.view.dom, { metaKey: true, button: 0 });
		expect(open).toHaveBeenCalledWith("https://xone.example.com", "_blank", "noreferrer");

		// Taking the definition away reads every block again.
		const now = editor.getText({ blockSeparator: "\n" });
		const start = posAt(now, now.indexOf("[spec]:"));
		editor.commands.deleteRange({ from: start, to: start + 1 });
		expect(blocksParsed(editor.state)).toBe(4);
		expect(links()).toBe(0);
		editor.commands.insertContentAt(start, "[");
		expect(blocksParsed(editor.state)).toBe(4);
		expect(links()).toBe(1);
		vi.restoreAllMocks();
		editor.destroy();
	});

	it("reads a definition in a fence as code, not as a definition", () => {
		const editor = noteEditor("[spec]\n\n```\n[spec]: https://x.co\n```");
		expect(editor.view.dom.querySelectorAll(".md-link")).toHaveLength(0);
		expect(blocksParsed(editor.state)).toBe(4);
		editor.destroy();
	});

	it("shows nothing at the caret while the editor has no focus", () => {
		const editor = noteEditor("# Title\n\nbody", true);
		expect(editor.state.selection.from).toBe(1);
		expect(hiddenIn(editor)).toEqual(["# "]);
		editor.view.focus();
		expect(hiddenIn(editor)).toEqual([]);
		editor.view.dom.blur();
		expect(hiddenIn(editor)).toEqual(["# "]);
		editor.destroy();
	});
});

describe("a note's blocks", () => {
	const details = "<details>\n<summary>Open</summary>\n\nHidden **x**\n\n</details>";

	it("holds an HTML block's blank lines in one block, and reads only it again", () => {
		const text = `intro\n\n${details}\n\nafter`;
		const editor = noteEditor(text);
		expect(blocksParsed(editor.state)).toBe(3);
		expect(editor.view.dom.querySelectorAll(".md-preview")).toHaveLength(1);
		expect(editor.view.dom.querySelectorAll("p.md-preview-source")).toHaveLength(6);

		editor.commands.setTextSelection(posAt(text, text.indexOf("Hidden") + 6));
		expect(editor.view.dom.querySelector(".md-preview")).toBeNull();
		editor.commands.insertContent("y");
		expect(blocksParsed(editor.state)).toBe(1);
		editor.commands.insertContent("\n");
		expect(blocksParsed(editor.state)).toBe(1);
		editor.destroy();
	});

	it("runs an unclosed HTML block on to the end, and draws nothing", () => {
		const editor = noteEditor("<details>\n\nfoo\n\nbar");
		expect(blocksParsed(editor.state)).toBe(1);
		expect(editor.view.dom.querySelector(".md-preview")).toBeNull();
		editor.destroy();
	});

	it("splits at blank lines again once the closing tag goes", () => {
		const text = `${details}\n\nafter`;
		const editor = noteEditor(text);
		expect(blocksParsed(editor.state)).toBe(2);
		const close = posAt(text, text.indexOf("</details>"));
		editor.commands.deleteRange({ from: close, to: close + "</details>".length });
		const fresh = noteEditor(editor.getText({ blockSeparator: "\n" }));
		expect(blocksParsed(fresh.state)).toBe(1);
		// The block it held runs to the end, so the whole of it reads again.
		editor.chain().setTextSelection(close).insertContent("</details>").run();
		expect(blocksParsed(editor.state)).toBe(2);
		const again = noteEditor(editor.getText({ blockSeparator: "\n" }));
		expect(blocksParsed(again.state)).toBe(2);
		again.commands.focus("end");
		expect(again.view.dom.querySelectorAll(".md-preview")).toHaveLength(1);
		for (const each of [editor, fresh, again]) each.destroy();
	});

	it("folds a rule into a line, and shows it with the caret on it", () => {
		const text = "a\n---\nb\n\n***\nc";
		const editor = noteEditor(text);
		const rule = () => editor.view.dom.querySelectorAll("p")[4]!;
		expect(rule().className).toBe("md-rule md-folded");
		editor.commands.setTextSelection(posAt(text, text.indexOf("***")));
		expect(rule().className).toBe("md-rule");
		const heading = editor.view.dom.querySelectorAll("p");
		expect(heading[0]!.className).toBe("md-h2");
		expect(heading[1]!.className).toBe("md-folded");
		editor.destroy();
	});
});

describe("the text box", () => {
	const hiddenIn = (textbox: HTMLElement) =>
		[...textbox.querySelectorAll(".md-hidden")].map((each) => each.textContent);

	/** Types into the text box with the focus in it, as typing has. */
	const typeIn = async (textbox: HTMLElement, text: string) => {
		act(() => editorOf(textbox).view.focus());
		await type(textbox, text);
	};

	it("draws the Markdown, and sends the text as it was typed", async () => {
		const { source } = renderComposer();
		const textbox = await findTextbox();
		await typeIn(textbox, "a **bold** word");

		expect(textbox.querySelector(".md-strong")).toHaveTextContent("bold");
		expect(textIn(textbox)).toBe("a **bold** word");
		fireEvent.keyDown(textbox, { key: "Enter" });

		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				{ text: "a **bold** word", attachments: [] },
				"queue"
			)
		);
	});

	it("hides markers once the caret is off them, and shows them again on it", async () => {
		renderComposer();
		const textbox = await findTextbox();
		await typeIn(textbox, "**bold** word");
		const editor = editorOf(textbox);
		expect(hiddenIn(textbox)).toEqual(["**", "**"]);

		act(() => void editor.commands.setTextSelection(4));
		expect(hiddenIn(textbox)).toEqual([]);

		act(() => void editor.commands.setTextSelection(9));
		expect(hiddenIn(textbox)).toEqual([]);

		act(() => void editor.commands.setTextSelection(11));
		expect(hiddenIn(textbox)).toEqual(["**", "**"]);
	});

	it("shows no markers at the caret while the text box has no focus", async () => {
		renderComposer();
		const textbox = await findTextbox();
		await type(textbox, "**bold** word");
		const editor = editorOf(textbox);
		act(() => editor.view.dom.blur());
		act(() => void editor.commands.setTextSelection(4));
		expect(hiddenIn(textbox)).toEqual(["**", "**"]);
		act(() => editor.view.focus());
		expect(hiddenIn(textbox)).toEqual([]);
	});

	it("draws a callout as the sent message will", async () => {
		renderComposer();
		const textbox = await findTextbox();
		await typeIn(textbox, "> [!WARNING]");
		act(() => void editorOf(textbox).commands.splitBlock());
		await type(textbox, "> Careful.");
		act(() => void editorOf(textbox).commands.splitBlock());
		await waitFor(() =>
			expect(textbox.querySelector(".md-preview")?.textContent?.trim()).toBe(
				"Warning\nCareful."
			)
		);
	});

	it("shows markers a selection reaches", async () => {
		renderComposer();
		const textbox = await findTextbox();
		await typeIn(textbox, "**bold** word");
		const editor = editorOf(textbox);

		act(() => void editor.commands.setTextSelection({ from: 5, to: 13 }));
		expect(hiddenIn(textbox)).toEqual([]);
	});

	it("keeps markers where they are while a press is down, and just after", async () => {
		renderComposer();
		const textbox = await findTextbox();
		await typeIn(textbox, "**bold** word");
		const editor = editorOf(textbox);
		editor.view.posAtCoords = () => null;

		fireEvent.mouseDown(textbox);
		act(() => void editor.commands.setTextSelection(4));
		expect(hiddenIn(textbox)).toEqual(["**", "**"]);

		fireEvent.mouseUp(window);
		expect(hiddenIn(textbox)).toEqual(["**", "**"]);
		await waitFor(() => expect(hiddenIn(textbox)).toEqual([]));
	});

	it("lets go of the markers when a drag ends, which has no release of its own", async () => {
		renderComposer();
		const textbox = await findTextbox();
		await typeIn(textbox, "**bold** word");
		const editor = editorOf(textbox);
		editor.view.posAtCoords = () => null;

		fireEvent.mouseDown(textbox);
		act(() => void editor.commands.setTextSelection(4));
		expect(hiddenIn(textbox)).toEqual(["**", "**"]);

		fireEvent.dragEnd(window);
		await waitFor(() => expect(hiddenIn(textbox)).toEqual([]));
	});

	it("adds a line on Enter in a fence, and still sends with Command+Enter", async () => {
		const { source } = renderComposer();
		const textbox = await findTextbox();
		await typeIn(textbox, "```");
		fireEvent.keyDown(textbox, { key: "Enter" });
		await typeIn(textbox, "code");
		fireEvent.keyDown(textbox, { key: "Enter" });

		expect(textIn(textbox)).toBe("```\ncode\n");
		expect(source.sendInput).not.toHaveBeenCalled();

		fireEvent.keyDown(textbox, { key: "Enter", metaKey: true });
		await waitFor(() => expect(source.sendInput).toHaveBeenCalledTimes(1));
	});

	it("sends on Enter once the fence is closed", async () => {
		const { source } = renderComposer();
		const textbox = await findTextbox();
		await typeIn(textbox, "```");
		fireEvent.keyDown(textbox, { key: "Enter" });
		await typeIn(textbox, "code");
		fireEvent.keyDown(textbox, { key: "Enter" });
		await typeIn(textbox, "```");
		fireEvent.keyDown(textbox, { key: "Enter" });

		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				{ text: "```\ncode\n```", attachments: [] },
				"queue"
			)
		);
	});

	/** Types each line, adding a line between them, with the caret left on the last. */
	const typeLines = async (textbox: HTMLElement, lines: readonly string[]) => {
		for (const [index, line] of lines.entries()) {
			if (index > 0) fireEvent.keyDown(textbox, { key: "Enter", shiftKey: true });
			await typeIn(textbox, line);
		}
	};
	const preview = (textbox: HTMLElement) => textbox.querySelector(".md-preview");
	const hiddenLines = (textbox: HTMLElement) =>
		[...textbox.querySelectorAll("p.md-preview-source")].map((line) => line.textContent);
	const MERMAID = ["```mermaid", "graph TD", "```"];

	it("draws a Mermaid block once the caret is off it, and sends it as typed", async () => {
		const { source } = renderComposer();
		const textbox = await findTextbox();
		await typeLines(textbox, MERMAID);
		expect(preview(textbox)).toBeNull();

		await typeLines(textbox, ["", "after"]);
		expect(hiddenLines(textbox)).toEqual(MERMAID);
		await waitFor(() => expect(preview(textbox)?.querySelector("img")).not.toBeNull());

		fireEvent.keyDown(textbox, { key: "Enter", metaKey: true });
		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				{ text: "```mermaid\ngraph TD\n```\nafter", attachments: [] },
				"queue"
			)
		);
	});

	it("shows a drawn block's source again when its drawing is pressed", async () => {
		renderComposer();
		const textbox = await findTextbox();
		await typeLines(textbox, [...MERMAID, "after"]);
		fireEvent.mouseDown(preview(textbox)!);

		expect(preview(textbox)).toBeNull();
		expect(hiddenLines(textbox)).toEqual([]);
		expect(editorOf(textbox).state.selection.$head.index(0)).toBe(1);
	});

	it("steps into a drawn block with the arrow keys, from either side", async () => {
		renderComposer();
		const textbox = await findTextbox();
		await typeLines(textbox, ["before", ...MERMAID, "after"]);
		const editor = editorOf(textbox);
		editor.view.endOfTextblock = () => true;
		expect(hiddenLines(textbox)).toEqual(MERMAID);

		fireEvent.keyDown(textbox, { key: "ArrowUp" });
		expect(editor.state.selection.$head.index(0)).toBe(3);
		expect(hiddenLines(textbox)).toEqual([]);

		act(() => void editor.commands.setTextSelection(3));
		expect(hiddenLines(textbox)).toEqual(MERMAID);
		fireEvent.keyDown(textbox, { key: "ArrowDown" });
		expect(editor.state.selection.$head.index(0)).toBe(1);
		expect(hiddenLines(textbox)).toEqual([]);
	});

	it("leaves the arrow keys alone away from the edge of a line", async () => {
		renderComposer();
		const textbox = await findTextbox();
		await typeLines(textbox, [...MERMAID, "after"]);
		const editor = editorOf(textbox);
		editor.view.endOfTextblock = () => false;

		fireEvent.keyDown(textbox, { key: "ArrowUp" });
		expect(editor.state.selection.$head.index(0)).toBe(3);
		expect(hiddenLines(textbox)).toEqual(MERMAID);
	});

	it("adds a line on Enter in a formula block", async () => {
		const { source } = renderComposer();
		const textbox = await findTextbox();
		await typeIn(textbox, "$$");
		fireEvent.keyDown(textbox, { key: "Enter" });
		await typeIn(textbox, "x^2");
		fireEvent.keyDown(textbox, { key: "Enter" });

		expect(textIn(textbox)).toBe("$$\nx^2\n");
		expect(source.sendInput).not.toHaveBeenCalled();
	});

	it("draws a formula in a sentence, and shows its source with the caret on it", async () => {
		renderComposer();
		const textbox = await findTextbox();
		await typeIn(textbox, "so $x^2$ holds");
		await waitFor(() => expect(textbox.querySelector(".md-formula .katex")).not.toBeNull());
		expect(hiddenIn(textbox)).toEqual(["$x^2$"]);

		act(() => void editorOf(textbox).commands.setTextSelection(6));
		expect(textbox.querySelector(".md-formula")).toBeNull();
		expect(hiddenIn(textbox)).toEqual([]);
		expect(textIn(textbox)).toBe("so $x^2$ holds");
	});

	it("shows a formula's source when its drawing is pressed", async () => {
		renderComposer();
		const textbox = await findTextbox();
		await typeIn(textbox, "so $x^2$ holds");
		fireEvent.mouseDown(textbox.querySelector(".md-formula")!);

		expect(textbox.querySelector(".md-formula")).toBeNull();
		expect(editorOf(textbox).state.selection.head).toBe(5);
	});

	it("sends on Enter after a one-line code span", async () => {
		const { source } = renderComposer();
		const textbox = await findTextbox();
		await typeIn(textbox, "```long snippet```");
		fireEvent.keyDown(textbox, { key: "Enter" });

		await waitFor(() => expect(source.sendInput).toHaveBeenCalledTimes(1));
	});
});
