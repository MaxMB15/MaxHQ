// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The text box's document as the shared draft sees it. Each line is a
// paragraph, and each pill is an inline node one position wide that reads
// as one object mark, so an offset in the draft's text and a position in
// the document only differ by the paragraph breaks.

import type { Fragment, Node as ProseNode, Schema } from "@tiptap/pm/model";

import { normalize, type PlacedPill, type Segment } from "@maxhq/shared/pills/draft";

/** The node each placed pill becomes. */
export const PILL_NODES = {
	attachment: "filePill",
	visual: "visualPill",
	quote: "quotePill",
	agent: "agentPill",
	note: "notePill",
	output: "outputPill",
} as const satisfies Record<PlacedPill["type"], string>;

/** The staged file a node's pill stands for, or null when it stands for none. */
export const fileKeyOfNode = (node: ProseNode): string | null => {
	if (node.type.name === PILL_NODES.attachment) return String(node.attrs.key);
	if (node.type.name === PILL_NODES.visual) return (node.attrs.ref as { key: string }).key;
	return null;
};

/** The pills' node names. */
export const PILL_NAMES: ReadonlySet<string> = new Set(Object.values(PILL_NODES));

/** Whether a node is one of the pills. */
export const isPill = (node: ProseNode | null | undefined): node is ProseNode =>
	node != null && PILL_NAMES.has(node.type.name);

/** The pill a node holds, or null for any other node. */
export const pillOf = (node: ProseNode): PlacedPill | null => {
	switch (node.type.name) {
		case PILL_NODES.attachment:
			return { type: "attachment", key: String(node.attrs.key) };
		case PILL_NODES.visual:
		case PILL_NODES.quote:
		case PILL_NODES.agent:
		case PILL_NODES.note:
		case PILL_NODES.output:
			return node.attrs.ref as PlacedPill;
		default:
			return null;
	}
};

/** The node for a pill. */
export const nodeFor = (schema: Schema, pill: PlacedPill): ProseNode =>
	pill.type === "attachment"
		? schema.nodes[PILL_NODES.attachment]!.create({ key: pill.key })
		: schema.nodes[PILL_NODES[pill.type]]!.create({ ref: pill });

const inlineSegments = (paragraph: ProseNode, out: Segment[]) =>
	paragraph.forEach((node) => {
		if (node.isText) out.push({ type: "text", text: node.text ?? "" });
		else {
			const pill = pillOf(node);
			if (pill) out.push({ type: "pill", pill });
		}
	});

/** The segments of a run of paragraphs, or of inline nodes from a selection. */
export const segmentsOfFragment = (fragment: Fragment): Segment[] => {
	const out: Segment[] = [];
	let lines = 0;
	fragment.forEach((node) => {
		if (node.isTextblock) {
			if (lines > 0) out.push({ type: "text", text: "\n" });
			lines += 1;
			inlineSegments(node, out);
		} else if (node.isText) out.push({ type: "text", text: node.text ?? "" });
		else {
			const pill = pillOf(node);
			if (pill) out.push({ type: "pill", pill });
		}
	});
	return normalize(out);
};

/** The document as the draft's segments, one "\n" between lines. */
export const segmentsOf = (doc: ProseNode): Segment[] => segmentsOfFragment(doc.content);

/** The document's text, one line per paragraph and each pill as one object mark. */
export const markedTextOf = (doc: ProseNode): string => {
	const lines: string[] = [];
	doc.forEach((paragraph) => lines.push(paragraph.textContent));
	return lines.join("\n");
};

/** The inline nodes of one line. */
const lineNodes = (schema: Schema, segments: readonly Segment[]): ProseNode[] =>
	segments.flatMap((segment) => {
		if (segment.type === "pill") return [nodeFor(schema, segment.pill)];
		return segment.text === "" ? [] : [schema.text(segment.text)];
	});

/** The paragraphs for segments, a new one at each "\n". */
export const paragraphsFor = (schema: Schema, segments: readonly Segment[]): ProseNode[] => {
	const lines: Segment[][] = [[]];
	for (const segment of segments) {
		if (segment.type === "pill") {
			lines.at(-1)!.push(segment);
			continue;
		}
		segment.text
			.replace(/\r\n?/g, "\n")
			.split("\n")
			.forEach((part, index) => {
				if (index > 0) lines.push([]);
				if (part !== "") lines.at(-1)!.push({ type: "text", text: part });
			});
	}
	return lines.map((line) => {
		const content = lineNodes(schema, line);
		return schema.nodes.paragraph!.create(null, content.length > 0 ? content : null);
	});
};

/**
 * The document position for an offset in the draft's text. Each line break
 * in the text is the two positions between paragraphs.
 */
export const posOf = (doc: ProseNode, offset: number): number => {
	let left = Math.max(0, offset);
	let pos = 0;
	let found = -1;
	doc.forEach((paragraph, at, index) => {
		if (found !== -1) return;
		const size = paragraph.content.size;
		if (left <= size || index === doc.childCount - 1) {
			found = at + 1 + Math.min(left, size);
			return;
		}
		left -= size + 1;
		pos = at + paragraph.nodeSize;
	});
	return found === -1 ? pos : found;
};

/** The offset in the draft's text for a document position. */
export const offsetOf = (doc: ProseNode, pos: number): number => {
	let offset = 0;
	let found = -1;
	doc.forEach((paragraph, at, index) => {
		if (found !== -1) return;
		const end = at + paragraph.nodeSize;
		if (pos < end || index === doc.childCount - 1) {
			found = offset + Math.max(0, Math.min(paragraph.content.size, pos - at - 1));
			return;
		}
		offset += paragraph.content.size + 1;
	});
	return found === -1 ? offset : found;
};
