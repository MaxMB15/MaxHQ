// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A code or text file, drawn read-only by the app itself, never by a page:
// HTML and the like show as their text. Colours come from the chat's
// highlighter, in the chat's code block colours. Line numbers sit in a
// gutter drawn by CSS, so a copy takes only the code. Lines never wrap.
//
// The lines go in blocks the browser lays out only near the screen, and a
// block takes its colours the first time it comes near, so a long file
// opens and scrolls without building every coloured run at once. Every
// line has the same height, which is how a block knows its size before it's
// drawn and how a line is found to scroll to.

import { t, useI18n } from "@maxhq/shared/i18n";

import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import {
	highlight,
	type Language,
	languageOf,
	type Lines,
	type Token,
} from "@/agent-view/highlight";
import { Empty } from "@/main-area/empty";
import { cn } from "@/lib/utils";
import { extensionOf, fileName } from "@maxhq/shared/agent-view/viewer-files";
import type { AgentId } from "@maxhq/shared/bindings";

import { readBytes } from "./load";
import { useReveal } from "./reveal";

/** How far into a file a zero byte marks it as binary. */
export const BINARY_SNIFF = 8 * 1024;

/** A file larger than this shows without colours. */
export const HIGHLIGHT_BYTES = 512 * 1024;

/** A file with more lines than this shows without colours. */
export const HIGHLIGHT_LINES = 10_000;

/** Each line's height, in pixels. */
const LINE = 20;

/** Lines in each block. */
const BLOCK = 200;

/** Space above the first line and below the last, in pixels. */
const PAD = 8;

/** What a file's bytes came to. */
export type Read =
	| { readonly kind: "binary" }
	| { readonly kind: "text"; readonly lines: readonly string[]; readonly bytes: number };

/**
 * Reads a file's bytes as text: binary when a zero byte comes in its first
 * 8 KB, and otherwise UTF-8, with a replacement character for anything that
 * isn't. A last line break doesn't make an empty line.
 */
export const readText = (bytes: ArrayBuffer): Read => {
	const view = new Uint8Array(bytes);
	if (view.subarray(0, BINARY_SNIFF).includes(0)) return { kind: "binary" };
	const text = new TextDecoder("utf-8").decode(view);
	const lines = text.split(/\r\n|\r|\n/);
	if (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
	return { kind: "text", lines, bytes: bytes.byteLength };
};

/** Whether a file this long takes colours. */
export const colourable = (read: { lines: readonly string[]; bytes: number }): boolean =>
	read.bytes <= HIGHLIGHT_BYTES && read.lines.length <= HIGHLIGHT_LINES;

/** What the pane says for a file it won't draw as text. */
export const binaryNote = (path: string): string =>
	t(
		"{{value1}} looks like a binary file, so it isn't shown as text. Open it in an editor with the Open in button above.",
		{ value1: fileName(path) }
	);

/** What the pane says for a code file too big to show. */
export const tooBigNote = (path: string, size: string): string =>
	t(
		"{{value1}} is {{size}}, too big to show here. Open it in an editor with the Open in button above.",
		{ value1: fileName(path), size: size }
	);

const tokenStyle = (token: Token) => ({
	color: token.color,
	fontStyle: token.fontStyle && token.fontStyle & 1 ? "italic" : undefined,
	fontWeight: token.fontStyle && token.fontStyle & 2 ? 600 : undefined,
});

// Calls back once `element` comes within a screen or so of `root`'s view.
// Where there's no IntersectionObserver, as in tests, that's at once.
const useNear = (
	element: React.RefObject<HTMLElement | null>,
	root: React.RefObject<HTMLElement | null>
): boolean => {
	const [near, setNear] = useState(typeof IntersectionObserver === "undefined");
	useEffect(() => {
		const at = element.current;
		if (near || !at) return;
		const watch = new IntersectionObserver(
			(seen) => {
				if (seen.some((entry) => entry.isIntersecting)) setNear(true);
			},
			{ root: root.current, rootMargin: "1200px 0px" }
		);
		watch.observe(at);
		return () => watch.disconnect();
	}, [element, root, near]);
	return near;
};

type BlockProps = {
	/** The number of the block's first line. */
	readonly first: number;
	readonly lines: readonly string[];
	/** The block's colours, when they're ready. */
	readonly colours: Lines | null;
	/** The marked line's number, when it's in this block. */
	readonly marked: number | null;
	/** Whether this block holds the file's last line. */
	readonly last: boolean;
	readonly gutter: string;
	readonly root: React.RefObject<HTMLElement | null>;
};

const Block = memo(({ first, lines, colours, marked, last, gutter, root }: BlockProps) => {
	useI18n();

	const box = useRef<HTMLDivElement>(null);
	const near = useNear(box, root);
	const shown = near ? colours : null;
	// Each line ends in a line break but the block's last, since the break
	// between blocks comes from the blocks themselves. That keeps a copy
	// across blocks to one break a line. An empty last line still takes one,
	// since a pre draws no line for a break it ends on.
	const end = (i: number) => (i < lines.length - 1 || lines[i] === "" ? "\n" : "");
	return (
		<div
			ref={box}
			className="flex"
			style={{
				contentVisibility: "auto",
				containIntrinsicSize: `auto ${lines.length * LINE}px`,
				paddingBottom: last ? PAD : undefined,
			}}
		>
			<div
				aria-hidden
				className="sticky left-0 z-[1] shrink-0 bg-subpanel pr-3 pl-3 text-right text-muted-foreground/55 select-none"
				style={{ width: gutter }}
			>
				{lines.map((_, i) => (
					<div
						key={i}
						data-n={first + i}
						className={cn(
							"h-5 before:content-[attr(data-n)]",
							first + i === marked && "text-foreground"
						)}
					/>
				))}
			</div>
			<pre className="m-0 pr-8 pl-2 font-mono whitespace-pre text-terminal select-text [tab-size:4]">
				{shown
					? lines.map((line, i) => (
							// Lines have no identity but their place.
							<span key={i} data-line={first + i}>
								{(shown[i] ?? [{ content: line }]).map((token, j) => (
									<span key={j} style={tokenStyle(token)}>
										{token.content}
									</span>
								))}
								{end(i)}
							</span>
						))
					: lines.map((line, i) => line + end(i)).join("")}
			</pre>
		</div>
	);
});
Block.displayName = "Block";

// The colours for the whole file, once the worker has them, or null.
const useColours = (lines: readonly string[], lang: Language | null, wanted: boolean) => {
	const [colours, setColours] = useState<Lines | null>(null);
	useEffect(() => {
		if (!lang || !wanted) return;
		let current = true;
		void highlight(lines.join("\n"), lang, Number.POSITIVE_INFINITY).then((found) => {
			if (current && found && found.length === lines.length) setColours(found);
		});
		return () => {
			current = false;
		};
	}, [lines, lang, wanted]);
	return colours;
};

/** A file's lines, numbered, coloured when it's short enough. */
export const CodeLines = ({
	agent,
	path,
	lines,
	bytes,
}: {
	agent: AgentId;
	path: string;
	lines: readonly string[];
	bytes: number;
}) => {
	const { t } = useI18n();

	const scroller = useRef<HTMLDivElement>(null);
	const lang = languageOf(extensionOf(path));
	const coloured = colourable({ lines, bytes });
	const colours = useColours(lines, lang, coloured);
	const reveal = useReveal(agent, path);
	const [marked, setMarked] = useState<number | null>(null);
	const blocks = useMemo(() => {
		const all: { first: number; lines: readonly string[] }[] = [];
		for (let at = 0; at < lines.length; at += BLOCK) {
			all.push({ first: at + 1, lines: lines.slice(at, at + BLOCK) });
		}
		return all;
	}, [lines]);
	const gutter = `calc(${String(lines.length).length}ch + 1.75rem)`;

	// Each ask to show a line scrolls it a third of the way down and marks it.
	useLayoutEffect(() => {
		const into = scroller.current;
		if (!reveal || !into) return;
		const line = Math.min(Math.max(1, reveal.line), lines.length);
		setMarked(line);
		into.scrollTop = Math.max(0, PAD + (line - 1) * LINE - into.clientHeight / 3);
	}, [reveal, lines.length]);

	return (
		<div className="flex h-full flex-col">
			{lang && !coloured ? (
				<p className="shrink-0 border-b border-border px-3 py-1.5 text-[11.5px] text-muted-foreground">
					{t("This file is long, so it shows without colours.")}
				</p>
			) : null}
			<div
				ref={scroller}
				role="region"
				aria-label={t("{{value1}}, read-only", { value1: fileName(path) })}
				// Focus lets the keys scroll it.
				tabIndex={0}
				className="min-h-0 flex-1 overflow-auto bg-subpanel font-mono text-[12px] leading-5 outline-none"
			>
				<div className="relative isolate w-max min-w-full" style={{ paddingTop: PAD }}>
					{marked !== null ? (
						<div
							data-marked={marked}
							className="pointer-events-none absolute inset-x-0 -z-10 bg-primary/15"
							style={{ top: PAD + (marked - 1) * LINE, height: LINE }}
						/>
					) : null}
					{blocks.map((block, i) => (
						<Block
							key={block.first}
							first={block.first}
							lines={block.lines}
							colours={
								colours
									? colours.slice(block.first - 1, block.first - 1 + BLOCK)
									: null
							}
							marked={
								marked !== null &&
								marked >= block.first &&
								marked < block.first + block.lines.length
									? marked
									: null
							}
							last={i === blocks.length - 1}
							gutter={gutter}
							root={scroller}
						/>
					))}
				</div>
			</div>
		</div>
	);
};

/** A code or text file from the media cache at `url`. */
export const CodeFile = ({
	agent,
	path,
	url,
	size,
}: {
	agent: AgentId;
	path: string;
	url: string;
	size: number;
}) => {
	const { t } = useI18n();

	const [read, setRead] = useState<Read | null>(null);
	const [failure, setFailure] = useState<string | null>(null);
	useEffect(() => {
		let current = true;
		readBytes(url, size).then(
			(bytes) => current && setRead(readText(bytes)),
			(error: unknown) => current && setFailure(String(error))
		);
		return () => {
			current = false;
		};
	}, [url, size]);
	if (failure) return <Empty>{failure}</Empty>;
	if (!read) return <Empty>{t("Reading {{value1}}…", { value1: fileName(path) })}</Empty>;
	if (read.kind === "binary") return <Empty>{binaryNote(path)}</Empty>;
	return <CodeLines agent={agent} path={path} lines={read.lines} bytes={read.bytes} />;
};
