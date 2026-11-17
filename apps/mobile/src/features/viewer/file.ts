// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the phone's file viewer makes of a file on an agent's device, apart
// from the views. The bytes come through the media cache, as the desktop's
// file pane gets them. Markdown draws as a reply does, and code, tables and
// plain text draw as numbered lines. Pictures draw as pictures, but an SVG
// shows as its text, since React Native's Image can't draw one. iOS shows a
// PDF in a web view, and Android, whose web view can't, says so. Models and
// Office documents only give their size, and Save or Share hands them on.
// Save downloads a file the viewer hasn't fetched whole, of any size, and
// says how far it has got.
//
// The phone can't use Show anyway: it fetches only from the agent's folders,
// so a file outside them says to show it on the Mac.

import { t } from "@maxhq/shared/i18n";

import { type DownloadState, downloadPercent } from "@maxhq/shared/agent-view/downloads";
import { formatBytes } from "@maxhq/shared/agent-view/format";
import type { Lines, Token } from "@maxhq/shared/agent-view/highlight";
import { FULL_LIMIT, type MediaState } from "@maxhq/shared/agent-view/media";
import { extensionOf, fileName, viewerKind } from "@maxhq/shared/agent-view/viewer-files";
import type { AgentId } from "@maxhq/shared/bindings";

import type { Screens } from "../../navigation";

/** How the phone draws a file. */
export type FileLook = "markdown" | "text" | "image" | "pdf" | "none";

/** How the phone draws the file at `path`, on iOS or another system. */
export const lookOf = (path: string, os: string): FileLook => {
	switch (viewerKind(path)) {
		case "markdown":
			return "markdown";
		case "code":
		case "table":
			return "text";
		case "image":
			return extensionOf(path) === "svg" ? "text" : "image";
		case "pdf":
			return os === "ios" ? "pdf" : "none";
		default:
			return "none";
	}
};

/** Where the File screen goes for a file, or null when the viewer doesn't open it. */
export const fileRoute = (
	agent: AgentId,
	path: string,
	line?: number | null
): Screens["File"] | null => {
	if (viewerKind(path) === null) return null;
	return line != null && line >= 1 ? { agent, path, line } : { agent, path };
};

/** What the screen says while a file isn't there to draw. */
export type FileNote =
	| { readonly kind: "reading"; readonly text: string }
	/** `more` when it can be fetched whole on a tap. */
	| { readonly kind: "large"; readonly text: string; readonly more: boolean }
	| { readonly kind: "failed"; readonly text: string };

/**
 * What the screen says for a file in this state, or null once it's ready.
 * A file outside the agent's folders says to show it on the Mac. `blocked`
 * is why Show anyway can't reach the device, which goes with any other
 * refused path, as on the desktop, since an older maxd calls one outside
 * the agent's folders a bad path.
 */
export const fileNote = (
	path: string,
	media: MediaState | null,
	blocked: string | null
): FileNote | null => {
	const name = fileName(path);
	switch (media?.state) {
		case undefined:
		case "loading":
			return {
				kind: "reading",
				text:
					media && media.total > 0
						? t("Reading {{name}}, {{value2}} of {{value3}}…", {
								name,
								value2: formatBytes(media.sent),
								value3: formatBytes(media.total),
							})
						: t("Reading {{name}}…", { name }),
			};
		case "large": {
			const size = formatBytes(media.size);
			return media.size <= FULL_LIMIT
				? {
						kind: "large",
						text: t("{{name}} is {{size}}, more than the phone opens on its own.", {
							name,
							size,
						}),
						more: true,
					}
				: {
						kind: "large",
						text: t("{{name}} is {{size}}, too large to show here.", { name, size }),
						more: false,
					};
		}
		case "failed": {
			const outside = media.refused === "outside";
			const said = outside
				? t("{{name}} is outside the agent's folders.", { name })
				: t("Couldn't read {{name}}. {{reason}}", { name, reason: media.reason });
			const why = outside
				? t("The phone can't show it. Use Show anyway on the Mac.")
				: media.refused
					? blocked
					: null;
			return { kind: "failed", text: why ? `${said} ${why}` : said };
		}
		case "ready":
			return null;
	}
};

/** What the screen says for a file it won't draw as text. */
export const binaryNote = (path: string): string =>
	t("{{name}} looks like a binary file, so it isn't shown as text.", { name: fileName(path) });

/** What the screen says for a file the phone can't draw. */
export const noViewNote = (path: string, size: number, os: string): string =>
	viewerKind(path) === "pdf" && os !== "ios"
		? t("Android can't show PDFs here. Save it to open it in another app.")
		: t("Can't show this file here. It's {{size}}.", { size: formatBytes(size) });

/** What the screen says while Save downloads the file, or null when it isn't downloading. */
export const saveText = (download: DownloadState | null): string | null => {
	if (download?.state !== "downloading") return null;
	if (download.held === "paused") return t("Paused, reconnecting");
	if (download.held === "waiting") return t("Waiting for room on this phone");
	const percent = downloadPercent(download.sent, download.total);
	return percent === null ? t("Downloading") : t("Downloading, {{percent}}%", { percent });
};

/** Lines in each block of the list. */
export const BLOCK = 200;

/** A run of a file's lines, drawn as one row of the list, with its colours when they're ready. */
export type LineBlock = {
	/** The number of the block's first line. */
	readonly first: number;
	readonly lines: readonly string[];
	readonly colours: Lines | null;
};

/** A file's lines in blocks of `size`, each with its share of the colours. */
export const lineBlocks = (
	lines: readonly string[],
	colours: Lines | null,
	size = BLOCK
): LineBlock[] => {
	const all: LineBlock[] = [];
	for (let at = 0; at < lines.length; at += size) {
		all.push({
			first: at + 1,
			lines: lines.slice(at, at + size),
			colours: colours ? colours.slice(at, at + size) : null,
		});
	}
	return all;
};

/**
 * The most characters of a line the list draws. A longer one, such as a
 * minified file's, ends in "…" there, as a native text view slows to a crawl
 * on a line that long.
 */
export const LONGEST = 2000;

/**
 * A line as the list draws it: a tab as four spaces, since a native text
 * view sets its own tab stops, and cut at LONGEST.
 */
export const shownLine = (line: string): string => {
	const flat = line.replace(/\t/g, "    ");
	return flat.length > LONGEST ? `${flat.slice(0, LONGEST)}…` : flat;
};

/** A line's colours as the list draws them, with its tabs and its cut as `shownLine` makes them. */
export const shownTokens = (tokens: readonly Token[]): readonly Token[] => {
	const shown: Token[] = [];
	let left = LONGEST;
	for (const token of tokens) {
		const content = token.content.replace(/\t/g, "    ");
		if (content.length > left) {
			shown.push({ ...token, content: `${content.slice(0, left)}…` });
			return shown;
		}
		shown.push(content === token.content ? token : { ...token, content });
		left -= content.length;
	}
	return shown;
};

// Characters from Hangul Jamo up, which take about two columns in a monospaced font.
const WIDE = /[ᄀ-￿]/;

/** About how many columns a monospaced font takes for `text`, counting a wide character as two. */
export const columnsOf = (text: string): number => {
	if (!WIDE.test(text)) return text.length;
	let columns = 0;
	for (const char of text) columns += (char.codePointAt(0) ?? 0) >= 0x1100 ? 2 : 1;
	return columns;
};

/** The most columns any of the lines takes as the list draws it. */
export const widestLine = (lines: readonly string[]): number => {
	let most = 0;
	for (const line of lines) most = Math.max(most, columnsOf(shownLine(line)));
	return most;
};

/**
 * The line to mark for a line asked for, kept inside the file, and how far
 * down to scroll so it sits a third of the way down a view `height` tall.
 */
export const scrollTo = (
	line: number,
	count: number,
	{ height, lineHeight, pad }: { height: number; lineHeight: number; pad: number }
): { readonly line: number; readonly offset: number } => {
	const marked = Math.min(Math.max(1, Math.floor(line)), Math.max(1, count));
	return { line: marked, offset: Math.max(0, pad + (marked - 1) * lineHeight - height / 3) };
};

const IMAGE_TYPES: Readonly<Record<string, string>> = {
	jpg: "image/jpeg",
	jpeg: "image/jpeg",
	svg: "image/svg+xml",
	tif: "image/tiff",
	ico: "image/x-icon",
};

const TYPES: Readonly<Record<string, string>> = {
	md: "text/markdown",
	markdown: "text/markdown",
	csv: "text/csv",
	tsv: "text/tab-separated-values",
	pdf: "application/pdf",
	html: "text/html",
	htm: "text/html",
	json: "application/json",
	docx: "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
	xlsx: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
	pptx: "application/vnd.openxmlformats-officedocument.presentationml.presentation",
	odt: "application/vnd.oasis.opendocument.text",
	ods: "application/vnd.oasis.opendocument.spreadsheet",
	odp: "application/vnd.oasis.opendocument.presentation",
	glb: "model/gltf-binary",
	gltf: "model/gltf+json",
	stl: "model/stl",
	obj: "model/obj",
	"3mf": "model/3mf",
};

/** The media type a saved copy of the file gets, from its extension. */
export const mimeOf = (path: string): string => {
	const extension = extensionOf(path);
	const kind = viewerKind(path);
	if (kind === "image") return IMAGE_TYPES[extension] ?? `image/${extension}`;
	return TYPES[extension] ?? (kind === "code" ? "text/plain" : "application/octet-stream");
};
