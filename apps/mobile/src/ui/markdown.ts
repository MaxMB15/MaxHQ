// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the native Markdown view gets besides the text: which links open, the
// phone's theme as Markdown styles, and each block made safe to show. The
// native view never loads an image: each one shows as a link to it, and one
// on the agent's device or in the text itself comes out as a picture to draw
// under the block. One on the web stays a link, since loading it would tell
// its server you read the reply. Raw HTML shows as text.

import { t } from "@maxhq/shared/i18n";

import { Lexer, type Token, type Tokens } from "marked";
import type { MarkdownStyle } from "react-native-enriched-markdown";

import { mathSegments } from "@maxhq/shared/agent-view/display-math";
import { splitBlocks } from "@maxhq/shared/agent-view/markdown";
import { type MediaSource, resolveMediaSource } from "@maxhq/shared/agent-view/media-source";

import { color, font, radius, size, tint } from "../theme";

/** A web address, as a pattern. The native view styles links by it too. */
export const WEB = "^[Hh][Tt][Tt][Pp][Ss]?://";
const web = new RegExp(WEB);

/** Whether a link opens, in the browser. Any other link stays text. */
export const opensInBrowser = (url: string): boolean => web.test(url);

// A backslash shows any ASCII punctuation as itself.
const literal = (text: string): string => text.replace(/[!-/:-@[-`{-~]/g, "\\$&");

/**
 * Text that shows as written, each line on its own. `gaps` are what sits
 * between the lines in the block, such as a quote's `> `.
 */
const asText = (raw: string, gaps: readonly string[] = []): string => {
	const lines = raw.split("\n").map((line) => literal(line.trimStart()));
	return lines.reduce(
		(text, line, i) => `${text}${lines[i - 1] && line ? "\\" : ""}${gaps[i - 1] ?? "\n"}${line}`
	);
};

const pattern = (text: string): string => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

type Found = { start: number; end: number; gaps: string[] };

// Where a token's source sits in the block, from `from` on. A token inside a
// quote or a list leaves out the `> ` or the indent its lines start with in
// the block, so each line break may have some of either after it.
const find = (block: string, raw: string, from: number): Found | null => {
	const search = new RegExp(raw.split("\n").map(pattern).join("(\\n[ \\t>]*)"), "g");
	search.lastIndex = from;
	const match = search.exec(block);
	return match && { start: match.index, end: search.lastIndex, gaps: match.slice(1) };
};

const inside = (token: Token): Token[] => {
	switch (token.type) {
		case "image":
			return [];
		case "list":
			return token.items;
		case "table":
			return [...token.header, ...token.rows.flat()].flatMap((cell) => cell.tokens);
		default:
			return "tokens" in token && token.tokens ? token.tokens : [];
	}
};

const plain = (tokens: readonly Token[]): string =>
	tokens
		.map((token) => {
			const within = inside(token);
			return within.length > 0 ? plain(within) : "text" in token ? token.text : "";
		})
		.join("");

const hasImage = (tokens: readonly Token[]): boolean =>
	tokens.some((token) => token.type === "image" || hasImage(inside(token)));

// Whether a ! sits just before `at` with no backslash escaping it.
const bareBangBefore = (block: string, at: number): boolean => {
	if (block[at - 1] !== "!") return false;
	let from = at - 1;
	while (block[from - 1] === "\\") from--;
	return (at - 1 - from) % 2 === 0;
};

/** A picture a block names: one the app can draw itself, with its alt text. */
export type BlockPicture = {
	readonly alt: string;
	readonly source: Extract<MediaSource, { access: "device" | "inline" }>;
};

/** A block as the feed shows it. */
export type SafeBlock = {
	/** What the native view gets. */
	readonly markdown: string;
	/** The pictures to draw under it. */
	readonly pictures: readonly BlockPicture[];
	/** Set when the pictures say all of it, so a reply can show them alone. */
	readonly onlyPictures: boolean;
};

const altOf = (image: Tokens.Image): string =>
	plain(image.tokens ?? [])
		.replace(/\s+/g, " ")
		.trim();

const pictureIn = (image: Tokens.Image): BlockPicture | null => {
	const source = resolveMediaSource(image.href, { imageEmbed: true });
	return source?.kind === "image" && source.access !== "web"
		? { alt: altOf(image), source }
		: null;
};

// Whether a block is one paragraph of nothing but the pictures it draws.
const allPictures = (tokens: readonly Token[], drawn: ReadonlySet<Token>): boolean => {
	const [only, ...more] = tokens.filter((token) => token.type !== "space");
	return (
		only?.type === "paragraph" &&
		more.length === 0 &&
		inside(only).every(
			(token) =>
				drawn.has(token) ||
				token.type === "br" ||
				(token.type === "text" && token.raw.trim() === "")
		)
	);
};

/**
 * A block as the native view gets it: each image a link to it, or its alt
 * text inside a link, and each HTML block its text, with the pictures it
 * names. A block with an image this can't place shows as text with no
 * pictures, so no image ever loads from something it can't read.
 */
export const safeBlock = (source: string): SafeBlock => {
	if (!source.includes("![") && !source.includes("<")) {
		return { markdown: source, pictures: [], onlyPictures: false };
	}
	const block = source.replace(/\r\n?/g, "\n");
	const edits: { start: number; end: number; text: string }[] = [];
	const pictures: BlockPicture[] = [];
	const drawn = new Set<Token>();
	let at = 0;
	let lost = false;
	const walk = (tokens: readonly Token[], inLink: boolean) => {
		for (const token of tokens) {
			const found = find(block, token.raw, at);
			const within = inside(token);
			if (within.length > 0) {
				if (found) at = found.start;
				walk(within, inLink || token.type === "link");
				if (found) at = Math.max(at, found.end);
			} else if (!found) {
				lost ||= token.type === "image";
			} else {
				at = found.end;
				if (token.type === "image") {
					const label = literal(altOf(token as Tokens.Image)) || t("Image");
					const picture = pictureIn(token as Tokens.Image);
					if (picture) {
						pictures.push(picture);
						drawn.add(token);
					}
					const text = inLink
						? label
						: `[${label}](<${token.href.replace(/[<>\\]/g, "\\$&")}>)`;
					// A ! just before would make the new link an image again.
					const bang = !inLink && bareBangBefore(block, found.start);
					edits.push(
						bang
							? { ...found, start: found.start - 1, text: `\\!${text}` }
							: { ...found, text }
					);
				} else if (token.type === "html" && token.block) {
					edits.push({ ...found, text: asText(token.raw, found.gaps) });
				}
			}
		}
	};
	const tokens = Lexer.lex(block);
	walk(tokens, false);
	if (lost) return { markdown: asText(block), pictures: [], onlyPictures: false };
	let out = "";
	let last = 0;
	for (const edit of edits) {
		out += block.slice(last, edit.start) + edit.text;
		last = edit.end;
	}
	const safe = out + block.slice(last);
	// The check that no image ever loads: anything still read as one shows as
	// text, with no pictures.
	if (hasImage(Lexer.lex(safe))) {
		return { markdown: asText(block), pictures: [], onlyPictures: false };
	}
	return {
		markdown: safe,
		pictures,
		onlyPictures: pictures.length > 0 && allPictures(tokens, drawn),
	};
};

/**
 * Text still streaming in, with a bar after its last word. A bar would turn a
 * closing fence, a table row or the `$$` line closing math into something
 * else, so those go without.
 */
export const withCaret = (text: string): string => {
	const trimmed = text.trimEnd();
	const last = trimmed.slice(trimmed.lastIndexOf("\n") + 1);
	return /^\s*(```|~~~|\||\$\$)/.test(last) ? trimmed : `${trimmed} ▍`;
};

/**
 * A piece of a Markdown text as the phone draws it: a top-level block for
 * the native view, or display math, which the native view can't draw.
 * `live` is set for math whose closing line hasn't come.
 */
export type MarkdownPiece =
	| { readonly kind: "block"; readonly source: string }
	| { readonly kind: "math"; readonly tex: string; readonly live: boolean };

/** `text` as its top-level blocks, with each `$$` block cut out on its own. */
export const markdownPieces = (text: string): MarkdownPiece[] =>
	mathSegments(text).flatMap((segment): MarkdownPiece[] =>
		segment.kind === "math"
			? [{ kind: "math", tex: segment.tex, live: !segment.closed }]
			: splitBlocks(segment.text).map((source) => ({ kind: "block", source }))
	);

/**
 * The phone's theme as Markdown styles: for a reply, or for your own message
 * in its bubble, where links keep the text's color.
 */
export const markdownStyle = (
	mine: boolean,
	hairline: number,
	fontScale: number,
	/** Set for quieter text, such as reasoning: smaller, and in this colour. */
	quiet?: string
): MarkdownStyle => {
	const body = {
		color: quiet ?? color.foreground,
		fontSize: quiet ? 12.5 : size.body,
		lineHeight: quiet ? 19 : mine ? 20 : 21,
		marginTop: 0,
		marginBottom: 8,
	};
	const heading = (fontSize: number) => ({
		...body,
		color: color.foregroundStrong,
		fontSize,
		fontWeight: "600",
		lineHeight: Math.round(fontSize * 1.35),
	});
	const box = { borderColor: color.border, borderWidth: hairline, borderRadius: radius.sm };
	return {
		paragraph: body,
		h1: heading(size.heading + 2),
		h2: heading(size.heading + 1),
		h3: heading(size.heading),
		h4: heading(size.body),
		h5: heading(size.body),
		h6: heading(size.body),
		strong: { color: color.foregroundStrong },
		link: { color: color.foreground, underline: false },
		...(mine ? {} : { linkVariants: { [WEB]: { color: color.primary } } }),
		code: {
			fontFamily: font.mono,
			fontSize: size.meta,
			color: color.terminal,
			backgroundColor: mine ? color.subpanel : color.secondary,
			borderColor: color.track,
		},
		codeBlock: {
			...body,
			...box,
			fontFamily: font.mono,
			fontSize: size.tiny,
			lineHeight: 17.5,
			color: color.terminal,
			backgroundColor: color.subpanel,
			padding: 10,
		},
		blockquote: {
			...body,
			color: color.mutedForeground,
			borderColor: color.track,
			borderWidth: 2,
			gapWidth: 12,
			backgroundColor: "transparent",
			admonitions: {
				note: { color: color.primary },
				tip: { color: color.ready },
				important: { color: color.linked },
				warning: { color: color.busy },
				caution: { color: color.error },
			},
		},
		list: {
			...body,
			bulletColor: color.mutedForeground,
			markerColor: color.mutedForeground,
			// The bullet sits on the gap's edge and grows with the system text size, so the gap does too.
			gapWidth: 12 * fontScale,
		},
		thematicBreak: { color: color.rowDivider, height: 1, marginTop: 4, marginBottom: 4 },
		table: {
			...body,
			...box,
			fontSize: size.small,
			lineHeight: 19,
			headerBackgroundColor: color.subpanel,
			headerTextColor: color.secondaryForeground,
			rowEvenBackgroundColor: "transparent",
			rowOddBackgroundColor: "transparent",
			cellPaddingHorizontal: 10,
			cellPaddingVertical: 6,
		},
		taskList: {
			checkedColor: color.primary,
			borderColor: color.mutedForeground,
			checkmarkColor: color.primaryForeground,
			checkedTextColor: color.foreground,
			checkedStrikethrough: false,
		},
		strikethrough: { color: color.mutedForeground },
		underline: { color: color.foreground },
		highlight: { color: color.foregroundStrong, backgroundColor: tint(color.busy, 0.3) },
		spoiler: { color: color.track },
	};
};
