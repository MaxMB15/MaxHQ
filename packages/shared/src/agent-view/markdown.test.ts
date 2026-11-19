// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { Lexer } from "marked";
import { describe, expect, it } from "vitest";

import { countLexing, longAnswer } from "../test/replay";
import { codeFenceOf, guardDollars, PLAIN_DOLLAR, restoreDollars, splitBlocks } from "./markdown";

const dollars = (text: string) => guardDollars(text).replaceAll(PLAIN_DOLLAR, "#");

describe("splitBlocks", () => {
	it("gives each top-level block its source, without the blank lines between", () => {
		expect(splitBlocks("# Title\n\nSome **bold** text.\n\n- one\n- two\n")).toEqual([
			"# Title",
			"Some **bold** text.",
			"- one\n- two\n",
		]);
	});

	it("keeps a fence whole, blank lines and all", () => {
		const fence = "```ts\nconst a = 1;\n\nconst b = 2;\n```";
		expect(splitBlocks(`Before.\n\n${fence}\n\nAfter.`)).toEqual(["Before.", fence, "After."]);
	});

	it("leaves an earlier block as it was while the text grows", () => {
		const first = splitBlocks("Done.\n\nStill wri");
		const later = splitBlocks("Done.\n\nStill writing this.");
		expect(later[0]).toBe(first[0]);
		expect(later).toHaveLength(2);
	});

	it("gives no blocks for empty text", () => {
		expect(splitBlocks("")).toEqual([]);
	});

	it("keeps a text whole when a block names a definition elsewhere", () => {
		const text =
			"See [the docs][docs] and[^1].\n\nMore.\n\n[docs]: https://example.com\n[^1]: A note.";
		expect(splitBlocks(text)).toEqual([text]);
	});

	it("keeps a text whole when raw HTML holds other blocks", () => {
		const text = "<details>\n<summary>More</summary>\n\n**Inside**\n\n</details>\n\nAfter.";
		expect(splitBlocks(text)).toEqual([text]);
	});

	it("keeps a text whole when math runs between `$$` lines", () => {
		const text = "Before.\n\n$$\na\n\nb\n$$\n";
		expect(splitBlocks(text)).toEqual([text]);
	});

	it("still splits around a comment and around HTML in a sentence", () => {
		expect(splitBlocks("<!-- note -->\n\nPress <kbd>Esc</kbd>.\n\nThen.")).toHaveLength(3);
	});
});

// The split as it was before it picked up from the text before, lexing all
// of the text each time. Streaming must give the same blocks.
const whole = (text: string): string[] => {
	if (/^[ >]*\[[^\]\n]+\]:/m.test(text) || /^ {0,3}\$\$/m.test(text)) return [text];
	const tokens = Lexer.lex(text);
	if (
		tokens.some((token) => token.type === "html" && !token.raw.trimStart().startsWith("<!--"))
	) {
		return [text];
	}
	return tokens.filter((token) => token.type !== "space").map((token) => token.raw);
};

const streams: [string, string][] = [
	["an open fence", "Run it:\n\n```ts\nconst a = 1;\n\n\nconst b = 2;\n```\n\nDone."],
	["a Mermaid fence", "Flow:\n\n```mermaid\ngraph TD;\n  A-->B;\n\n  B-->C;\n```\nAfter."],
	[
		"a growing list",
		"Steps:\n\n- one\n- two\n  - two a\n\n  more of two\n- three\n\n1. first\n2. second\n\nEnd.",
	],
	[
		"a table",
		"Costs:\n\n| Step | Cost |\n| --- | ---: |\n| Reset | Once |\n| Batch | Per frame |\nAfter.\n\nNext.",
	],
	["math", "Inline $x^2$ here.\n\nThen:\n\n$$\na = b\n\nc = d\n$$\n\nAfter."],
	["a setext heading", "Title\n=====\n\nSub\nline\n---\n\nText."],
	["a rule that becomes text", "para\n***x and more\n***\n\n---\n\nend"],
	["a rule between lines a heading underlines", "para\n***\nmore\n===\n\nend\n\n***\n"],
	[
		"raw HTML",
		"Before.\n\n<details>\n<summary>More</summary>\n\nInside.\n\n</details>\n\nAfter.",
	],
	["a quote with a lazy line", "> quoted\nlazy line\n> back\n\n> next\n\nAfter."],
	["indented code", "para\n    not code\n\n    code\n    more\n\nafter"],
	["Windows line ends", "One.\r\n\r\n- a\r\n- b\r\n\r\n```\r\ncode\r\n```\r\nEnd.\r\n"],
	["a definition", "See [docs][d].\n\nMore.\n\n[d]: https://example.com\n\nEnd."],
];

describe("splitBlocks while a text streams in", () => {
	it.each(streams)(
		"gives the same blocks as the whole text, a character at a time: %s",
		(_, text) => {
			for (let end = 0; end <= text.length; end++) {
				const prefix = text.slice(0, end);
				expect(splitBlocks(prefix), JSON.stringify(prefix)).toEqual(whole(prefix));
			}
		}
	);

	it.each(streams)(
		"gives the same blocks as the whole text, a few characters at a time: %s",
		(_, text) => {
			for (let end = 0; end < text.length + 7; end += 7) {
				const prefix = text.slice(0, end);
				expect(splitBlocks(prefix), JSON.stringify(prefix)).toEqual(whole(prefix));
			}
		}
	);

	it("gives the same blocks for each of several texts streaming at once", () => {
		const texts = streams.map(([, text]) => text);
		const longest = Math.max(...texts.map((text) => text.length));
		for (let end = 0; end <= longest; end += 3) {
			for (const text of texts) {
				const prefix = text.slice(0, end);
				expect(splitBlocks(prefix), JSON.stringify(prefix)).toEqual(whole(prefix));
			}
		}
	});

	it("gives the same blocks for texts of lines that change what comes before them", () => {
		const lines = [
			"para",
			"text *em*",
			"***",
			"---",
			"___",
			"===",
			"-",
			"*",
			"- item",
			"* star",
			"1. one",
			"2) two",
			"  - nested",
			"    indented",
			"```",
			"```mermaid",
			"~~~",
			"> quote",
			">",
			"# head",
			"| a | b |",
			"| - | - |",
			"a | b",
			"-|-",
			"<div>",
			"</div>",
			"<!-- c -->",
			"  ",
			"",
			"",
			"",
		];
		// The same texts every run, so a failure comes back.
		let seed = 11;
		const random = (below: number) => {
			seed = (seed * 1_103_515_245 + 12_345) % 2_147_483_648;
			return seed % below;
		};
		for (let n = 0; n < 150; n++) {
			const text = Array.from(
				{ length: 3 + random(14) },
				() => lines[random(lines.length)]
			).join("\n");
			const step = 1 + random(4);
			for (let end = 0; end <= text.length; end += step) {
				const prefix = text.slice(0, end);
				expect(splitBlocks(prefix), JSON.stringify(prefix)).toEqual(whole(prefix));
			}
		}
	});

	it("lexes again only from the block before the last as a long text grows", () => {
		const text = longAnswer(20_000);
		splitBlocks(text.slice(0, 10_000));
		const { lexing, stop } = countLexing();
		for (let end = 10_009; end < 12_000; end += 9) splitBlocks(text.slice(0, end));
		stop();
		// Each section's last blocks are a few hundred characters at most.
		expect(lexing.chars / lexing.calls).toBeLessThan(1_000);
	});
});

describe("guardDollars", () => {
	it.each([
		["$x^2$", "$x^2$"],
		["Then $a + b$, and $c$.", "Then $a + b$, and $c$."],
		["($x$)", "($x$)"],
		["$a\nb$", "$a\nb$"],
		["$$x$$", "$$x$$"],
		["$$\nx\n$$", "$$\nx\n$$"],
	])("leaves math alone: %j", (text, shown) => {
		expect(dollars(text)).toBe(shown);
	});

	it.each([
		["$5 and $10", "#5 and #10"],
		["$5-$10", "#5-#10"],
		["**$5** and **$10**", "**#5** and **#10**"],
		["US$5 and C$6", "US#5 and C#6"],
		["$HOME and $PATH", "#HOME and #PATH"],
		["echo $HOME", "echo #HOME"],
		["It costs $5.", "It costs #5."],
		["$ x$", "# x#"],
		["a$b$c", "a#b#c"],
		["$x$y", "#x#y"],
		["$one\n\ntwo$", "#one\n\ntwo#"],
	])("keeps a dollar that isn't math as text: %j", (text, shown) => {
		expect(dollars(text)).toBe(shown);
	});

	it("skips an escaped dollar", () => {
		expect(dollars("\\$5 and $x$")).toBe("\\$5 and $x$");
		expect(dollars("$a\\$b$")).toBe("$a\\$b$");
	});

	it("keeps every offset, and comes back as it was", () => {
		const text = "$5 🙂 and `$x` and $y$ for $10";
		expect(guardDollars(text)).toHaveLength(text.length);
		expect(restoreDollars(guardDollars(text))).toBe(text);
	});

	it("gives back a text with no dollar as it is", () => {
		expect(guardDollars("nothing here")).toBe("nothing here");
	});
});

describe("codeFenceOf", () => {
	it("reads a fence's language and code", () => {
		expect(codeFenceOf("```ts\nconst a = 1;\n\nlet b;\n```\n")).toEqual({
			lang: "ts",
			code: "const a = 1;\n\nlet b;",
			closed: true,
		});
		expect(codeFenceOf("~~~~ python extra words\nx = 1\n~~~~")).toEqual({
			lang: "python",
			code: "x = 1",
			closed: true,
		});
	});

	it("gives no language for a bare fence", () => {
		expect(codeFenceOf("```\nplain\n```")).toEqual({ lang: null, code: "plain", closed: true });
	});

	it("closes only on as many of the same marks", () => {
		expect(codeFenceOf("````\n```\n~~~~\n````")).toMatchObject({
			code: "```\n~~~~",
			closed: true,
		});
	});

	it("keeps a fence that hasn't closed open, as one streaming in", () => {
		expect(codeFenceOf("```rust\nfn main() {\n")).toEqual({
			lang: "rust",
			code: "fn main() {",
			closed: false,
		});
		expect(codeFenceOf("```")).toEqual({ lang: null, code: "", closed: false });
	});

	it("takes the opening line's indent off each line", () => {
		expect(codeFenceOf("  ```sh\n    ls\n ls -a\n  ```")).toMatchObject({
			code: "  ls\nls -a",
		});
	});

	it("matches how the lexer splits fences", () => {
		const text = "Intro\n\n```go\nfunc main() {}\n```\n\n~~~\nplain\n~~~\n";
		const fences = splitBlocks(text).map(codeFenceOf);
		expect(fences).toEqual([
			null,
			{ lang: "go", code: "func main() {}", closed: true },
			{ lang: null, code: "plain", closed: true },
		]);
	});

	it("leaves anything but a lone fence alone", () => {
		expect(codeFenceOf("text\n```\ncode\n```")).toBeNull();
		expect(codeFenceOf("```\ncode\n```\nmore text")).toBeNull();
		expect(codeFenceOf("    indented code")).toBeNull();
		expect(codeFenceOf("``` a`b\ncode")).toBeNull();
		expect(codeFenceOf("``inline``")).toBeNull();
	});

	it("reads Windows line breaks", () => {
		expect(codeFenceOf("```js\r\na\r\n```\r\n")).toMatchObject({ lang: "js", code: "a" });
	});
});
