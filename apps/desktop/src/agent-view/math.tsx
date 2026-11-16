// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Math in a message. KaTeX and its fonts load with the first formula that
// shows, and stay, so a row scrolling back into view draws at once. KaTeX
// reads its input without trusting it: a formula can't name a link or a
// picture, and one it can't read shows in red.

import { useEffect, useState } from "react";

import { mathSettings } from "@maxhq/shared/agent-view/drawing";

type Katex = typeof import("katex").default;

let katex: Katex | null = null;
let loading: Promise<Katex> | null = null;

/** Brings in KaTeX and its look, once. */
export const loadMath = (): Promise<Katex> => {
	loading ??= Promise.all([import("katex"), import("katex/dist/katex.min.css")]).then(
		([module]) => {
			katex = module.default;
			return module.default;
		}
	);
	return loading;
};

/**
 * A formula as HTML. It's null until KaTeX has loaded, and for one too deep
 * for it to read, whatever `throwOnError` says, so its source shows instead.
 */
export const mathHtml = (source: string, display: boolean): string | null => {
	try {
		return (
			katex?.renderToString(source, mathSettings(display, "html", "var(--destructive)")) ??
			null
		);
	} catch {
		return null;
	}
};

/**
 * A formula, in the sentence or on its own lines. Its source stands for it
 * until KaTeX loads, and for a reader. Find and quotes leave it out.
 */
export const MathView = ({ source, display = false }: { source: string; display?: boolean }) => {
	const [, setLoaded] = useState(false);
	const html = mathHtml(source, display);
	useEffect(() => {
		if (html !== null) return;
		let current = true;
		void loadMath().then(() => {
			if (current) setLoaded(true);
		});
		return () => {
			current = false;
		};
	}, [html]);

	if (html === null)
		return display ? (
			<div data-find-skip className="font-mono text-[11.5px] text-muted-foreground">
				{source}
			</div>
		) : (
			<code data-find-skip className="font-mono text-[11.5px] text-muted-foreground">
				{source}
			</code>
		);
	return display ? (
		<div
			data-find-skip
			role="img"
			aria-label={source}
			className="overflow-x-auto py-1 [&_.katex-display]:m-0"
		>
			<div aria-hidden dangerouslySetInnerHTML={{ __html: html }} />
		</div>
	) : (
		<span data-find-skip role="img" aria-label={source} className="formula-box">
			<span aria-hidden dangerouslySetInnerHTML={{ __html: html }} />
		</span>
	);
};
