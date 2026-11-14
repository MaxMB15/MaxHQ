// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What every device knows about a visual's page apart from drawing it: how
// large it may be, the key its drawn height and its hang mark are kept by,
// and the file it saves as.

import { fnv1a } from "../lib/fnv";

/** The most a visual's page may hold, in UTF-8 bytes. */
export const MAX_VISUAL_BYTES = 512 * 1024;

/** The tallest a visual draws, in CSS pixels. */
export const MAX_VISUAL_HEIGHT = 20_000;

/** How many UTF-8 bytes a page takes. */
export const visualBytes = (html: string): number => {
	// One byte per code unit, and more for the units past ASCII. A surrogate
	// pair is two units for four bytes, which is two more, as for any unit
	// past U+07FF.
	let bytes = html.length;
	for (let i = 0; i < html.length; i++) {
		const unit = html.charCodeAt(i);
		if (unit >= 0x800) bytes += 2;
		else if (unit >= 0x80) bytes += 1;
		if ((unit & 0xfc00) === 0xd800 && (html.charCodeAt(i + 1) & 0xfc00) === 0xdc00) i++;
	}
	return bytes;
};

/**
 * A short key for a page, the same on every device: its length and its
 * FNV-1a, both in base 36. Two pages sharing one is unlikely enough for a
 * height cache and a hang mark.
 */
export const visualKey = (html: string): string =>
	`${html.length.toString(36)}-${fnv1a(html).toString(36)}`;

const MOST_SLUG = 60;

/** The file a visual saves as: `visual-<title as a slug>.html`. */
export const visualFileName = (title: string): string => {
	const slug = title
		.normalize("NFKD")
		.replace(/[̀-ͯ]/g, "")
		.toLowerCase()
		.replace(/[^a-z0-9]+/g, "-")
		.slice(0, MOST_SLUG)
		.replace(/^-+|-+$/g, "");
	return slug ? `visual-${slug}.html` : "visual.html";
};
