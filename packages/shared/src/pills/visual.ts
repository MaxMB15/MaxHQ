// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A visual as a composer's pill: the page travels as a staged file, the
// way a large paste does, and the pill keeps its title and where it came
// from. A copy or a drag carries the page itself, so a paste in any
// composer stages it again. Plain text pastes as a visual only when it's
// exactly one visual's fenced block, and a page over the limit stays text.

import { fromVisualFence } from "../visuals/fence";
import { readVisualTransfer, VISUAL_MIME, type VisualTransfer } from "../visuals/transfer";
import { MAX_VISUAL_BYTES, visualBytes, visualFileName } from "../visuals/visual";

import type { ClipboardFile, ClipboardFragment } from "./clipboard";
import type { VisualPill } from "./draft";

/** The origin of a fragment made from a visual, which no composer shares. */
export const VISUAL_ORIGIN = "visual";

/** The key a fragment made from one visual names its page by. */
export const VISUAL_KEY = "visual";

/** Where a visual came from, as its pill's preview names it. */
export const visualFrom = (agentTitle: string, deviceName: string | null): string =>
	deviceName ? `${agentTitle} on ${deviceName}` : agentTitle;

/** Whether a page is small enough to go as a visual. */
export const fitsVisual = (html: string): boolean => visualBytes(html) <= MAX_VISUAL_BYTES;

/** A visual's page as the file a fragment carries it in. */
export const visualFile = (key: string, title: string, page: string): ClipboardFile => ({
	key,
	name: visualFileName(title),
	kind: "file",
	source: null,
	ref: null,
	device: null,
	page,
});

/** One visual as a fragment: its pill, and its page as the file it names. */
export const visualFragment = (
	{ html, title, from }: VisualTransfer,
	origin = VISUAL_ORIGIN
): ClipboardFragment => {
	const pill: VisualPill = { type: "visual", key: VISUAL_KEY, title, from };
	return {
		version: 1,
		origin,
		segments: [{ type: "pill", pill }],
		files: [visualFile(VISUAL_KEY, title, html)],
		gitItems: [],
	};
};

/**
 * The visual a paste or a drop carries: under its own type, or as plain
 * text that is exactly one visual's fenced block. A page over the limit
 * gives null, so it goes as text.
 */
export const readVisual = (data: {
	readonly getData: (type: string) => string;
}): VisualTransfer | null => {
	const carried = data.getData(VISUAL_MIME);
	if (carried) return readVisualTransfer(carried);
	const fenced = fromVisualFence(data.getData("text/plain"));
	return fenced && fitsVisual(fenced.html) ? { ...fenced, from: null } : null;
};
