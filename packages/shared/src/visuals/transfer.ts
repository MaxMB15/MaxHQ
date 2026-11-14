// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A visual on its way between places in the app: dragged from a card to a
// composer or a note, or copied in a MaxHQ clipboard type. Any page can
// put text under any type, so a read checks every field and the size.

import { MAX_VISUAL_BYTES, visualBytes } from "./visual";
import type { Visual } from "./fence";

/** The drag and clipboard type a visual travels under. */
export const VISUAL_MIME = "application/x-maxhq-visual";

/** A visual and where it came from, such as "Token audit on Mac mini". */
export type VisualTransfer = Visual & { readonly from: string | null };

/** The text a visual travels as. */
export const writeVisualTransfer = ({ html, title, from }: VisualTransfer): string =>
	JSON.stringify({ html, title, from });

/** The visual in a transfer's text, or null when it isn't one or is too large. */
export const readVisualTransfer = (text: string): VisualTransfer | null => {
	let value: unknown;
	try {
		value = JSON.parse(text);
	} catch {
		return null;
	}
	if (typeof value !== "object" || value === null) return null;
	const { html, title, from } = value as Record<string, unknown>;
	if (typeof html !== "string" || typeof title !== "string") return null;
	if (from !== null && typeof from !== "string") return null;
	if (visualBytes(html) > MAX_VISUAL_BYTES) return null;
	return { html, title, from };
};
