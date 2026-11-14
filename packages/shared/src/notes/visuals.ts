// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A visual put into a note's text: as its fenced block on lines of its own,
// with a blank line on each side so it reads as a block of its own, at a
// place in the text or at its end. Each placing says exactly what it put
// where, so Undo can take that block out again. A visual past its size
// limit, or one that would take the note past its own, is refused with a
// sentence that says so.

import { t, formatNumber } from "../i18n";

import type { AgentId, ProjectId } from "../bindings";
import type { ViewState } from "../view/replica";
import { type Visual, toVisualFence } from "../visuals/fence";
import { readVisualTransfer, VISUAL_MIME } from "../visuals/transfer";
import { MAX_VISUAL_BYTES, visualBytes } from "../visuals/visual";

/** The most a note's text may hold, in UTF-8 bytes, as `MAX_NOTE_BODY` in the protocol. */
export const MAX_NOTE_BODY = 1024 * 1024;

/** Text put into a note: `text` at offset `at`. */
export type Placed = { readonly at: number; readonly text: string };

/** A placing, or why it was refused. */
export type Placing =
	{ readonly placed: Placed; readonly body: string } | { readonly refused: string };

/** A size the way the limits are written, such as "512 KB" or "1 MB". */
const sizeText = (bytes: number): string => {
	if (bytes < 1024 * 1024)
		return t("{{value}} KB", { value: formatNumber(Math.ceil(bytes / 1024)) });
	const mb = bytes / (1024 * 1024);
	return t("{{value}} MB", {
		value: Number.isInteger(mb)
			? formatNumber(mb)
			: formatNumber(mb, { minimumFractionDigits: 1, maximumFractionDigits: 1 }),
	});
};

/** Why a visual can't go into a note for its size alone, or null when it can. */
export const visualTooLarge = (html: string): string | null => {
	const bytes = visualBytes(html);
	return bytes > MAX_VISUAL_BYTES
		? t("This visual is {{size}}, and a visual holds at most {{most}}.", {
				size: sizeText(bytes),
				most: sizeText(MAX_VISUAL_BYTES),
			})
		: null;
};

/** Why a note's text can't grow to `body`, or null when it can. */
export const noteTooLarge = (body: string): string | null => {
	const bytes = visualBytes(body);
	return bytes > MAX_NOTE_BODY
		? t("This would make the note {{size}}, and a note holds at most {{most}}.", {
				size: sizeText(bytes),
				most: sizeText(MAX_NOTE_BODY),
			})
		: null;
};

/** What goes around a block put at `at`, so it has lines of its own and blank lines around it. */
const padding = (body: string, at: number): { before: string; after: string } => {
	const head = body.slice(0, at);
	const tail = body.slice(at);
	const before = head === "" || head.endsWith("\n\n") ? "" : head.endsWith("\n") ? "\n" : "\n\n";
	const after =
		tail === "" ? "\n" : tail.startsWith("\n\n") ? "" : tail.startsWith("\n") ? "\n" : "\n\n";
	return { before, after };
};

/** Puts `block`, some lines of Markdown, into `body` at offset `at`. */
export const placeBlock = (body: string, at: number, block: string): Placing => {
	const where = Math.max(0, Math.min(at, body.length));
	const { before, after } = padding(body, where);
	const text = `${before}${block}${after}`;
	const next = body.slice(0, where) + text + body.slice(where);
	const refused = noteTooLarge(next);
	return refused ? { refused } : { placed: { at: where, text }, body: next };
};

/** Puts a visual's fenced block into `body` at offset `at`, or says why it can't. */
export const placeVisual = (body: string, at: number, visual: Visual): Placing => {
	const refused = visualTooLarge(visual.html);
	return refused ? { refused } : placeBlock(body, at, toVisualFence(visual));
};

/** Puts a visual's fenced block at the end of `body`, or says why it can't. */
export const appendVisual = (body: string, visual: Visual): Placing =>
	placeVisual(body, body.length, visual);

/**
 * `body` with what was placed taken out again: where it was put, or where
 * it is the only copy of it. Null when it was changed or can't be told
 * apart from another copy.
 */
export const takeOut = (body: string, placed: Placed): string | null => {
	const where = placedAt(body, placed);
	return where < 0 ? null : body.slice(0, where) + body.slice(where + placed.text.length);
};

/**
 * Where what was placed is in `body` now: where it was put, or where it is
 * the only copy of it. -1 when it was changed or has a twin.
 */
export const placedAt = (body: string, { at, text }: Placed): number => {
	if (body.slice(at, at + text.length) === text) return at;
	const first = body.indexOf(text);
	return first === body.lastIndexOf(text) ? first : -1;
};

/** The project an agent works in, through its workspace, or null. */
export const projectOfAgent = (view: ViewState, agent: AgentId): ProjectId | null => {
	const workspace = view.tables.agent.get(agent)?.workspace;
	return (workspace === undefined ? null : view.tables.workspace.get(workspace)?.project) ?? null;
};

/**
 * The visual a paste or a drop carries in the MaxHQ type: the visual, why
 * it can't go in for its size, or null when it carries none.
 */
export const transferredVisual = (data: {
	getData: (type: string) => string;
}): { readonly visual: Visual } | { readonly refused: string } | null => {
	const raw = data.getData(VISUAL_MIME);
	if (!raw) return null;
	const visual = readVisualTransfer(raw);
	if (visual) return { visual: { html: visual.html, title: visual.title } };
	// A transfer is refused for its size too, which is worth saying.
	try {
		const { html } = JSON.parse(raw) as { html?: unknown };
		const refused = typeof html === "string" ? visualTooLarge(html) : null;
		if (refused) return { refused };
	} catch {
		// Not a transfer at all.
	}
	return null;
};
