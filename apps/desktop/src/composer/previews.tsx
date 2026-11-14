// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the text box shows in place of Markdown it can't draw over its own
// characters: a diagram, a formula, a table, a callout or, in a note, a
// picture or raw HTML such as `<details>`. They use the feed's own views, so
// what you see here is what the message shows once sent. ProseMirror owns the
// text box, so each one sits in a small DOM node of its own.

import { createRoot, type Root } from "react-dom/client";

import { DiagramPreview } from "@/agent-view/diagram";
import { Markdown, type Written } from "@/agent-view/markdown";
import { loadMath, mathHtml, MathView } from "@/agent-view/math";

/** The blocks that draw whole. */
export type BlockKind = "mermaid" | "math" | "table" | "callout" | "image" | "html";

// A table with a pill keeps its source, so no piece ever draws from here.
const WRITTEN: Written = { draw: () => null, plain: () => null };

const Shown = ({ kind, source, reply }: { kind: BlockKind; source: string; reply: boolean }) => {
	switch (kind) {
		case "mermaid":
			return <DiagramPreview source={source} />;
		case "math":
			return <MathView source={source} display />;
		case "table":
		case "image":
			return <Markdown text={source} written={WRITTEN} />;
		// A note draws a callout as a reply would, and the composer as a
		// message you send.
		case "callout":
			return <Markdown text={source} written={reply ? undefined : WRITTEN} />;
		// Only a note draws HTML, and as a reply would, through its allowlist.
		case "html":
			return <Markdown text={source} />;
	}
};

const roots = new WeakMap<Node, Root>();

/** A block as it will read, in a node ProseMirror can place. With `reply`, as a reply draws it. */
export const blockPreview = (kind: BlockKind, source: string, reply = false): HTMLElement => {
	const dom = document.createElement("div");
	dom.className = "md-preview";
	dom.contentEditable = "false";
	const root = createRoot(dom);
	roots.set(dom, root);
	root.render(<Shown kind={kind} source={source} reply={reply} />);
	return dom;
};

/** Lets go of what `blockPreview` drew. ProseMirror may redraw during a render, so it waits. */
export const removeBlockPreview = (dom: Node) => {
	const root = roots.get(dom);
	if (root) setTimeout(() => root.unmount());
};

/** A formula as it will read in its sentence, or its source until KaTeX loads. */
export const formulaPreview = (source: string): HTMLElement => {
	const dom = document.createElement("span");
	dom.className = "formula-box md-formula";
	dom.contentEditable = "false";
	dom.setAttribute("role", "img");
	dom.setAttribute("aria-label", source);
	const draw = () => {
		const html = mathHtml(source, false);
		if (html === null) dom.textContent = source;
		else dom.innerHTML = html;
		return html !== null;
	};
	if (!draw()) void loadMath().then(draw);
	return dom;
};
