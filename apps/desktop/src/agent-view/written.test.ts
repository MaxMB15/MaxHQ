// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { pieceMark } from "@maxhq/shared/pills/sent";
import remarkParse from "remark-parse";
import { unified } from "unified";
import { describe, expect, it } from "vitest";

import { remarkWritten } from "./written";

// Piece 1 is a pill, and nothing else is.
const plain = (index: number) => (index === 1 ? "[Image 1: a.png]" : null);
const PILL = pieceMark(1);

type Shown = { type: string; value?: string; children?: Shown[]; data?: unknown };

const treeOf = (source: string): Shown => {
	const parser = unified().use(remarkParse);
	const tree = parser.parse(source) as unknown as Shown;
	remarkWritten(plain)(tree as never);
	return tree;
};

/** What a node holds, in short: a text's value, or a node's type with its children. */
const short = (node: Shown): unknown =>
	node.type === "text"
		? node.value
		: node.children
			? [node.type, ...node.children.map(short)]
			: node.value !== undefined
				? [node.type, node.value]
				: node.type;

const shown = (source: string) => (treeOf(source).children ?? []).map(short);

describe("remarkWritten", () => {
	it("keeps each line break", () => {
		expect(shown("one\ntwo")).toEqual([["paragraph", "one", "break", "two"]]);
	});

	it("puts each pill where its mark is, with its index", () => {
		const paragraph = treeOf(`see ${PILL} now`).children![0]!;
		expect(paragraph.children!.map(short)).toEqual(["see ", "sentPiece", " now"]);
		expect(paragraph.children![1]!.data).toEqual({
			hName: "span",
			hProperties: { dataPiece: 1 },
		});
	});

	it("reads a pill inside bold", () => {
		expect(shown(`**a ${PILL}**`)).toEqual([["paragraph", ["strong", "a ", "sentPiece"]]]);
	});

	it("leaves a mark with no pill as the text it is", () => {
		const stray = pieceMark(5);
		expect(shown(`a ${stray}`)).toEqual([["paragraph", "a ", stray]]);
	});

	it("keeps raw HTML as text, in a sentence and as a block", () => {
		expect(shown("use <Foo> here")).toEqual([["paragraph", "use ", "<Foo>", " here"]]);
		expect(shown("<div>\nx\n</div>")).toEqual([
			["paragraph", "<div>", "break", "x", "break", "</div>"],
		]);
		expect(shown("> <div>")).toEqual([["blockquote", ["paragraph", "<div>"]]]);
	});

	it("shows a pill in code as its plain text", () => {
		expect(shown(`\`a ${PILL}\``)).toEqual([
			["paragraph", ["inlineCode", "a [Image 1: a.png]"]],
		]);
		expect(shown(`\`\`\`\n${PILL}\n\`\`\``)).toEqual([["code", "[Image 1: a.png]"]]);
	});
});
