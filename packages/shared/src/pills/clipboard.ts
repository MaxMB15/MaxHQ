// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: a copy keeps its
// pills as a fragment of the draft in a MaxHQ
// clipboard type, and in the HTML too, since some paste paths only keep
// HTML. The plain text beside it is what other apps get. A hand check
// stands in for a schema library. Any app or web page can put HTML
// on the clipboard, so a fragment read from HTML keeps no files: only this
// app can write its own type, and only that one may name a file to stage.

import type { AgentId, AttachmentKind, AttachmentRef, DeviceId } from "../bindings";
import { VENDOR_NAME } from "../status/usage";

import {
	fileMarker,
	fileNumbers,
	type GitItemRef,
	normalize,
	type PlacedPill,
	type Segment,
	visualMarker,
} from "./draft";

/** The clipboard type that carries a copied piece of a draft. */
export const DRAFT_CLIPBOARD_MIME = "application/x-maxhq-draft";

/** A file a copied pill names, so a paste in another composer can add it again. */
export type ClipboardFile = {
	readonly key: string;
	readonly name: string;
	readonly kind: AttachmentKind | "missing";
	/** The path on this Mac it came from, if it came from one. */
	readonly source: string | null;
	/** The staged attachment and the device it staged on, once it has. */
	readonly ref: AttachmentRef | null;
	readonly device: DeviceId | null;
	/** Where an agent can see the file, such as one in a sent message. */
	readonly found?: ClipboardFound | null;
	/** A visual's page itself, so a paste anywhere can stage it again. */
	readonly page?: string;
};

/** A file in an agent's reach, on the agent's device. */
export type ClipboardFound = {
	readonly agent: AgentId;
	readonly device: DeviceId;
	readonly path: string;
};

/** A copied piece of a draft. */
export type ClipboardFragment = {
	readonly version: 1;
	/** The composer it came from, so a paste there reuses its files. */
	readonly origin: string;
	readonly segments: readonly Segment[];
	readonly files: readonly ClipboardFile[];
	readonly gitItems: readonly GitItemRef[];
};

const MAX_FRAGMENT_CHARS = 16_000_000;

const HTML_WRAPPER_SLACK_CHARS = 4096;

/** The fragment as clipboard text, or null when it's too big to carry. */
export const encodeFragment = (fragment: ClipboardFragment): string | null => {
	const encoded = JSON.stringify(fragment);
	return encoded.length <= MAX_FRAGMENT_CHARS ? encoded : null;
};

const escapeHtml = (text: string) =>
	text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

/** HTML that shows the plain text and carries the fragment. */
export const encodeClipboardHtml = (text: string, fragment: string): string =>
	`<pre data-maxhq-draft="${encodeURIComponent(fragment)}">${escapeHtml(text)}</pre>`;

/**
 * The fragment with each file as its marker text and no files left, since
 * a fragment from HTML may come from any app or web page. A visual whose
 * page goes along keeps its pill, with only the page: a page runs in a
 * sandbox wherever it came from, while a path or a staged file could name
 * anything.
 */
const withoutFiles = (fragment: ClipboardFragment): ClipboardFragment => {
	const files = new Map(fragment.files.map((file) => [file.key, file]));
	const numbers = fileNumbers(fragment.segments, (key) => files.get(key));
	const kept: ClipboardFile[] = [];
	const segments = fragment.segments.map((segment): Segment => {
		if (segment.type !== "pill") return segment;
		const { pill } = segment;
		if (pill.type === "visual") {
			const file = files.get(pill.key);
			if (file?.page === undefined) return { type: "text", text: visualMarker(pill.title) };
			const { key, name, page } = file;
			kept.push({ key, name, kind: "file", source: null, ref: null, device: null, page });
			return segment;
		}
		if (pill.type !== "attachment") return segment;
		const file = files.get(pill.key);
		const text = file ? fileMarker(file, numbers.get(pill.key)) : "";
		return { type: "text", text };
	});
	return { ...fragment, segments: normalize(segments), files: kept };
};

/**
 * The fragment inside copied HTML, or null when it has none. It keeps its
 * text, agents, quotes and pull requests, and each file becomes its marker.
 */
export const decodeClipboardHtml = (html: string | null | undefined): ClipboardFragment | null => {
	// Percent encoding can expand each UTF-16 code unit to nine characters.
	if (!html || html.length > MAX_FRAGMENT_CHARS * 9 + HTML_WRAPPER_SLACK_CHARS) return null;
	const encoded = /data-maxhq-draft=["']([^"']+)["']/.exec(html)?.[1];
	if (!encoded) return null;
	try {
		const fragment = decodeFragment(decodeURIComponent(encoded));
		return fragment && withoutFiles(fragment);
	} catch {
		return null;
	}
};

const isRecord = (value: unknown): value is Record<string, unknown> =>
	typeof value === "object" && value !== null && !Array.isArray(value);

const isString = (value: unknown): value is string => typeof value === "string";

const isNullableString = (value: unknown): value is string | null =>
	value === null || isString(value);

const isPosition = (value: unknown): value is number =>
	typeof value === "number" && Number.isInteger(value) && value >= 0 && value <= 0xffff_ffff;

/** Whether a value is a pill that takes one place in the text. */
export const isPill = (value: unknown): value is PlacedPill => {
	if (!isRecord(value)) return false;
	switch (value.type) {
		case "attachment":
			return isString(value.key);
		case "visual":
			return isString(value.key) && isString(value.title) && isNullableString(value.from);
		case "quote":
			return (
				isString(value.agent) &&
				isString(value.agent_title) &&
				isString(value.item) &&
				isString(value.text) &&
				isPosition(value.position) &&
				isString(value.source) &&
				isNullableString(value.from)
			);
		case "agent":
			return (
				isString(value.agent) &&
				isString(value.title) &&
				isString(value.vendor) &&
				Object.hasOwn(VENDOR_NAME, value.vendor) &&
				isString(value.device) &&
				isNullableString(value.excerpt)
			);
		case "note":
			return (
				isFilled(value.project) &&
				isFilled(value.note) &&
				isString(value.title) &&
				isNullableString(value.copy)
			);
		default:
			return false;
	}
};

const isSegment = (value: unknown): value is Segment =>
	isRecord(value) &&
	((value.type === "text" && isString(value.text)) ||
		(value.type === "pill" && isPill(value.pill)));

const isFilled = (value: unknown): value is string => isString(value) && value !== "";

const isFound = (value: unknown): value is ClipboardFound | null | undefined =>
	value === undefined ||
	value === null ||
	(isRecord(value) && isFilled(value.agent) && isFilled(value.device) && isFilled(value.path));

const FILE_KINDS: ReadonlySet<unknown> = new Set<ClipboardFile["kind"]>([
	"image",
	"pdf",
	"file",
	"folder",
	"missing",
]);

const isFile = (value: unknown): value is ClipboardFile =>
	isRecord(value) &&
	isString(value.key) &&
	isString(value.name) &&
	FILE_KINDS.has(value.kind) &&
	isNullableString(value.source) &&
	(value.ref === null || (isRecord(value.ref) && isString(value.ref.id))) &&
	isNullableString(value.device) &&
	isFound(value.found) &&
	(value.page === undefined || isString(value.page));

const isGitItem = (value: unknown): value is GitItemRef =>
	isRecord(value) &&
	value.type === "git_item" &&
	isString(value.repo) &&
	typeof value.number === "number" &&
	isString(value.title) &&
	isString(value.state) &&
	isString(value.url);

/** Whether a value is a fragment, as parsed JSON gives it. */
export const isFragment = (value: unknown): value is ClipboardFragment =>
	isRecord(value) &&
	value.version === 1 &&
	isString(value.origin) &&
	Array.isArray(value.segments) &&
	value.segments.every(isSegment) &&
	Array.isArray(value.files) &&
	value.files.every(isFile) &&
	Array.isArray(value.gitItems) &&
	value.gitItems.every(isGitItem);

/** The fragment in clipboard text, or null when it isn't one. */
export const decodeFragment = (raw: string | null | undefined): ClipboardFragment | null => {
	if (!raw || raw.length > MAX_FRAGMENT_CHARS) return null;
	try {
		const value: unknown = JSON.parse(raw);
		return isFragment(value) ? value : null;
	} catch {
		return null;
	}
};

/**
 * The fragment a paste carries, from its own type or from its HTML. Only
 * the MaxHQ type keeps files: WebKit shows a custom type only to the
 * origin that wrote it, while HTML can come from anywhere.
 */
export const readFragment = (data: {
	getData: (type: string) => string;
}): ClipboardFragment | null =>
	decodeFragment(data.getData(DRAFT_CLIPBOARD_MIME)) ??
	decodeClipboardHtml(data.getData("text/html"));
