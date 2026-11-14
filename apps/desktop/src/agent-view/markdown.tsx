// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Markdown for replies and plans. The text splits into top-level blocks and
// each block renders on its own, so a reply streaming in only renders its
// last block again. A fence named `mermaid` draws its diagram, `$` and `$$`
// and a fence named `math` draw math, and a quote that starts with `[!NOTE]`
// is a callout. Raw HTML in a reply shows only what an allowlist keeps, such
// as `<details>` and `<kbd>`. An image shows the picture or video it names on
// the agent's device, through the media cache, and one on the web shows as a
// link to open in the browser. A fence named `html visual` draws its page
// as a visual once the fence has closed. A message you wrote also has its
// line breaks, its raw HTML as text, and its pills in place.

import { createContext, memo, type ReactNode, use, useId, useMemo } from "react";
import ReactMarkdown, {
	type Components,
	defaultUrlTransform,
	type ExtraProps,
	type UrlTransform,
} from "react-markdown";
import rehypeRaw from "rehype-raw";
import rehypeSanitize from "rehype-sanitize";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import type { PluggableList } from "unified";

import { guardDollars, splitBlocks } from "@maxhq/shared/agent-view/markdown";
import { ErrorBoundary } from "@/components/error-boundary";
import { cn } from "@/lib/utils";
import { VisualCard } from "@/visuals/visual-card";
import { VisualStreaming } from "@/visuals/visual-streaming";

import { AlertTitle, alertEdge } from "./alert";
import { CodeBlock } from "./code-block";
import { Diagram } from "./diagram";
import {
	htmlSchema,
	remarkAlerts,
	remarkLiteralTags,
	remarkPlainDollars,
	remarkVisualFences,
} from "./markdown-plugins";
import { MarkdownMedia } from "./markdown-media";
import { MathView } from "./math";
import { PictureSet } from "./picture-set";
import { type PiecePlain, remarkWritten } from "./written";

// Just enough of a syntax tree node to read a code block's text and language.
type TreeNode = {
	readonly type: string;
	readonly value?: string;
	readonly tagName?: string;
	readonly properties?: {
		readonly className?: unknown;
		readonly dataVisual?: unknown;
		readonly dataOpen?: unknown;
	};
	readonly children?: readonly TreeNode[];
};

const textOf = (node: TreeNode): string =>
	node.type === "text" ? (node.value ?? "") : (node.children ?? []).map(textOf).join("");

const fenceLanguage = (node: TreeNode | undefined): string | null => {
	const names = node?.properties?.className;
	if (!Array.isArray(names)) return null;
	const name = names.find((n): n is string => typeof n === "string" && n.startsWith("language-"));
	return name ? name.slice("language-".length) : null;
};

const LINK = "text-primary hover:underline";

const isMath = (className: string | undefined): boolean =>
	className?.split(" ").includes("language-math") ?? false;

// What an element keeps of the props react-markdown hands it, which also
// carry the tree node they came from.
const attributes = <T extends { node?: unknown }>(props: T) => {
	const kept: { node?: unknown } = { ...props };
	delete kept.node;
	return kept as Omit<T, "node">;
};

// Whether the block is the last of a message still streaming in.
const Live = createContext(false);

// The same component for a block whether it's live or not, so a visual
// keeps its frame as its message finishes.
const Pre = ({ node }: ExtraProps) => {
	const live = use(Live);
	const block = node as unknown as TreeNode | undefined;
	const code = block?.children?.find((child) => child.tagName === "code");
	// A `<pre>` in raw HTML can hold its text with no `<code>`.
	const held = code ?? block;
	const text = held ? textOf(held) : "";
	const lang = fenceLanguage(code);
	const name = lang?.toLowerCase();
	const visual = code?.properties?.dataVisual;
	if (typeof visual === "string") {
		const html = text.replace(/\n$/, "");
		if (code?.properties?.dataOpen === undefined)
			return <VisualCard className="my-1" html={html} title={visual} />;
		if (live) return <VisualStreaming className="my-1" html={html} title={visual} />;
	}
	if (name === "mermaid") return <Diagram className="my-1" source={text} live={live} />;
	if (name === "math" && !live) return <MathView source={text} display />;
	return <CodeBlock className="my-1" code={text} lang={lang} live={live} />;
};

const LOOK: Components = {
	pre: Pre,
	code: ({ children, className }) =>
		isMath(className) ? (
			<MathView source={String(children)} />
		) : (
			<code className="rounded-[4px] border border-track bg-secondary px-[5px] py-px font-mono text-[11.5px] text-terminal">
				{children}
			</code>
		),
	// A link within the message, as to a footnote, moves to it there.
	a: ({ href, children, ...props }) =>
		href?.startsWith("#") ? (
			<a
				{...attributes(props)}
				href={href}
				className={LINK}
				onClick={(event) => {
					event.preventDefault();
					document
						.getElementById(decodeURIComponent(href.slice(1)))
						?.scrollIntoView({ block: "nearest" });
				}}
			>
				{children}
			</a>
		) : (
			<a {...attributes(props)} href={href} target="_blank" rel="noreferrer" className={LINK}>
				{children}
			</a>
		),
	// Find leaves a picture out, and the text on either side runs on.
	img: ({ src, alt }) => (
		<span data-find-skip className="contents">
			<MarkdownMedia src={typeof src === "string" ? src : undefined} alt={alt} />
		</span>
	),
	p: ({ children }) => <p>{children}</p>,
	ul: ({ children, className }) => (
		<ul
			className={cn(
				"flex flex-col gap-1",
				className?.includes("contains-task-list") ? "list-none pl-1" : "list-disc pl-5"
			)}
		>
			{children}
		</ul>
	),
	ol: ({ children }) => <ol className="flex list-decimal flex-col gap-1 pl-5">{children}</ol>,
	li: ({ children, id }) => (
		<li id={id} className="pl-0.5 marker:text-muted-foreground">
			{children}
		</li>
	),
	h1: ({ children }) => (
		<h3 className="text-[15px] font-semibold text-foreground-strong">{children}</h3>
	),
	// The footnotes' own heading is for a reader, as find and quotes take it.
	h2: ({ children, className }) =>
		className === "sr-only" ? (
			<h2 className="sr-only">{children}</h2>
		) : (
			<h3 className="text-[14px] font-semibold text-foreground-strong">{children}</h3>
		),
	h3: ({ children }) => (
		<h4 className="text-[13.5px] font-semibold text-foreground-strong">{children}</h4>
	),
	h4: ({ children }) => (
		<h5 className="text-[13px] font-semibold text-foreground-strong">{children}</h5>
	),
	h5: ({ children }) => (
		<h6 className="text-[13px] font-medium text-foreground-strong">{children}</h6>
	),
	h6: ({ children }) => (
		<h6 className="text-[12.5px] font-medium text-muted-foreground">{children}</h6>
	),
	blockquote: ({ node, children }) => {
		const kind = node?.properties.dataAlert;
		return typeof kind === "string" ? (
			<blockquote className={cn("flex flex-col gap-1 border-l-2 pl-3", alertEdge(kind))}>
				{children}
			</blockquote>
		) : (
			<blockquote className="border-l-2 border-track pl-3 text-muted-foreground">
				{children}
			</blockquote>
		);
	},
	div: ({ node, children, ...rest }) => {
		const kind = node?.properties.dataAlertTitle;
		return typeof kind === "string" ? (
			<AlertTitle kind={kind}>{children}</AlertTitle>
		) : (
			<div {...rest}>{children}</div>
		);
	},
	section: ({ children, className, ...props }) => (
		<section
			{...attributes(props)}
			className={cn(
				className?.includes("footnotes") &&
					"border-t border-row-divider pt-2 text-[12px] text-muted-foreground"
			)}
		>
			{children}
		</section>
	),
	details: ({ children, ...props }) => (
		<details
			{...attributes(props)}
			className="flex flex-col gap-1 rounded-md border border-border px-2.5 py-1.5"
		>
			{children}
		</details>
	),
	summary: ({ children }) => (
		<summary className="cursor-pointer font-medium text-foreground-strong">{children}</summary>
	),
	kbd: ({ children }) => (
		<kbd className="rounded-[4px] border border-input bg-secondary px-[5px] py-px font-mono text-[11px] text-terminal">
			{children}
		</kbd>
	),
	mark: ({ children }) => (
		<mark className="rounded-[3px] bg-status-busy/25 px-0.5 text-inherit">{children}</mark>
	),
	dt: ({ children }) => <dt className="font-semibold text-foreground-strong">{children}</dt>,
	dd: ({ children }) => <dd className="pl-4">{children}</dd>,
	hr: () => <hr className="border-row-divider" />,
	table: ({ children }) => (
		<div className="overflow-x-auto rounded-md border border-border">
			<table className="w-full border-collapse text-[12.5px]">{children}</table>
		</div>
	),
	th: ({ children, style }) => (
		<th
			style={style}
			className="border-b border-border bg-subpanel px-2.5 py-1.5 text-left font-semibold text-secondary-foreground"
		>
			{children}
		</th>
	),
	td: ({ children, style }) => (
		<td style={style} className="border-t border-row-divider px-2.5 py-1.5 align-top">
			{children}
		</td>
	),
	input: ({ checked }) => (
		<input
			type="checkbox"
			checked={checked}
			readOnly
			disabled
			className="mr-1.5 align-middle"
		/>
	),
};

/** The pills of a message you wrote, by the index of each one's mark in its text. */
export type Written = {
	readonly draw: (index: number) => ReactNode;
	readonly plain: PiecePlain;
};

// Through context, so a pill's clock doesn't parse the text again.
const DrawPiece = createContext<Written["draw"] | null>(null);

const Piece = ({ node }: ExtraProps) => {
	const draw = use(DrawPiece);
	const index = node?.properties.dataPiece;
	return draw && typeof index === "number" ? draw(index) : null;
};

const WRITTEN: Components = { ...LOOK, span: Piece };
const PLUGINS: PluggableList = [
	remarkGfm,
	remarkMath,
	remarkPlainDollars,
	remarkAlerts,
	remarkVisualFences,
];

// Whether a block may hold raw HTML, which parsing it again costs time to learn.
const MAY_HOLD_HTML = /<[a-zA-Z!/]/;

// Quotes nested deeper than anyone writes, which overflow the stack of the
// steps that walk the tree. The block shows as written, without a parse
// that would only fail.
const TOO_DEEP = /^(?:[ \t]*>){100}/m;

// An image's target goes to MarkdownMedia as written, which shows only what
// it can vouch for, such as a path or a data picture the default drops.
// Every other URL keeps the default's check.
const urlTransform: UrlTransform = (url, key, node) =>
	node.tagName === "img" && key === "src" ? url : defaultUrlTransform(url);

const Block = memo(
	({
		source,
		look,
		live,
		plugins,
		ids,
		html,
	}: {
		source: string;
		look: Components;
		/** Set while the block is the last of a message still streaming in. */
		live: boolean;
		plugins: PluggableList;
		/** What starts each id this message's footnotes give an element. */
		ids: string;
		/** Whether raw HTML shows what the allowlist keeps, and otherwise none of it. */
		html: boolean;
	}) => {
		const raw = html && MAY_HOLD_HTML.test(source);
		const remarkPlugins = useMemo<PluggableList>(
			() => (raw ? [...plugins, remarkLiteralTags] : plugins),
			[raw, plugins]
		);
		const rehypePlugins = useMemo<PluggableList>(
			() => (raw ? [rehypeRaw, [rehypeSanitize, htmlSchema(ids)]] : []),
			[raw, ids]
		);
		const remarkRehypeOptions = useMemo(() => ({ clobberPrefix: ids }), [ids]);
		if (TOO_DEEP.test(source)) return <p className="whitespace-pre-wrap">{source}</p>;
		return (
			<Live value={live}>
				<ReactMarkdown
					remarkPlugins={remarkPlugins}
					rehypePlugins={rehypePlugins}
					remarkRehypeOptions={remarkRehypeOptions}
					components={look}
					urlTransform={urlTransform}
					skipHtml
				>
					{guardDollars(source)}
				</ReactMarkdown>
			</Live>
		);
	}
);
Block.displayName = "MarkdownBlock";

/** Markdown text, rendered block by block. */
export const Markdown = ({
	text,
	streaming = false,
	className,
	after,
	written,
}: {
	text: string;
	/** Set while the text still grows, so its last block skips highlighting. */
	streaming?: boolean;
	className?: string;
	/** Shown after the last block, such as a streaming caret. */
	after?: ReactNode;
	/** Set when the text is a message you wrote, with marks for its pills. */
	written?: Written;
}) => {
	const blocks = useMemo(() => splitBlocks(text), [text]);
	const plain = written?.plain;
	const plugins = useMemo<PluggableList>(
		() => (plain ? [...PLUGINS, [remarkWritten, plain]] : PLUGINS),
		[plain]
	);
	// Footnotes link by id, so each message's are its own.
	const ids = `md-${useId().replace(/[^a-zA-Z0-9]/g, "")}-`;
	return (
		<div
			// Every Markdown in a row is text find counts.
			data-find="markdown"
			className={cn(
				"flex min-w-0 flex-col gap-2 text-[13px] leading-[1.55] break-words text-foreground select-text",
				className
			)}
		>
			<PictureSet>
				<DrawPiece value={written?.draw ?? null}>
					{blocks.map((source, i) => (
						// A block's place is its identity: earlier blocks never change while text streams in.
						<ErrorBoundary
							key={i}
							part="Markdown block"
							fallback={<p className="whitespace-pre-wrap">{source}</p>}
							resetOn={source}
						>
							<Block
								source={source}
								look={written ? WRITTEN : LOOK}
								live={!written && streaming && i === blocks.length - 1}
								plugins={plugins}
								ids={ids}
								html={!written}
							/>
						</ErrorBoundary>
					))}
				</DrawPiece>
			</PictureSet>
			{after}
		</div>
	);
};
