// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A message you wrote reads as Markdown the way a reply does, apart from
// three things. A line break stays one, as it was in the text box. Raw HTML
// stays text, since it was typed to be read, such as `<Foo>` in a sentence.
// And each pill's mark becomes the pill, which code shows as plain text.

import { pieceIndexOf } from "@maxhq/shared/pills/sent";

/** A piece as plain text by its index, or null when no pill has that index. */
export type PiecePlain = (index: number) => string | null;

// Just enough of a syntax tree node to rewrite a message's text.
type TreeNode = {
	type: string;
	value?: string;
	children?: TreeNode[];
	data?: {
		hName?: string;
		hProperties?: { dataPiece: number };
		hChildren?: TreeNode[];
	};
};

const LINE_OR_MARK = /(\n|[\u{100000}-\u{10FFFD}])/u;
const MARKS = /[\u{100000}-\u{10FFFD}]/gu;

/** What holds whole blocks, so text put there needs a paragraph of its own. */
const HOLDS_BLOCKS = new Set(["root", "blockquote", "listItem", "footnoteDefinition"]);

const inlineNodes = (value: string, plain: PiecePlain): TreeNode[] =>
	value.split(LINE_OR_MARK).flatMap((part): TreeNode[] => {
		if (part === "") return [];
		if (part === "\n") return [{ type: "break" }];
		const index = pieceIndexOf(part);
		return index !== null && plain(index) !== null
			? [{ type: "sentPiece", data: { hName: "span", hProperties: { dataPiece: index } } }]
			: [{ type: "text", value: part }];
	});

const plainCode = (value: string, plain: PiecePlain): string =>
	value.replace(MARKS, (mark) => plain(pieceIndexOf(mark) ?? -1) ?? mark);

// A formula also keeps its text where it goes on to render from.
const plainMath = (node: TreeNode, plain: PiecePlain) => {
	if (node.value !== undefined) node.value = plainCode(node.value, plain);
	node.children?.forEach((child) => plainMath(child, plain));
	node.data?.hChildren?.forEach((child) => plainMath(child, plain));
};

const rewrite = (node: TreeNode, plain: PiecePlain) => {
	node.children = node.children?.flatMap((child): TreeNode[] => {
		switch (child.type) {
			case "text":
				return inlineNodes(child.value ?? "", plain);
			case "html": {
				const children = inlineNodes(child.value ?? "", plain);
				return HOLDS_BLOCKS.has(node.type) ? [{ type: "paragraph", children }] : children;
			}
			case "code":
			case "inlineCode":
				return [{ ...child, value: plainCode(child.value ?? "", plain) }];
			case "math":
			case "inlineMath":
				plainMath(child, plain);
				return [child];
			default:
				rewrite(child, plain);
				return [child];
		}
	});
};

/** A remark plugin that reads a syntax tree as a message you wrote. */
export const remarkWritten = (plain: PiecePlain) => (tree: TreeNode) => rewrite(tree, plain);
