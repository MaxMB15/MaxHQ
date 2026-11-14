// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a feed draws of the pictures a row carries. A file's preview shows
// only in the form maxd makes one, so a device can't make a client load an
// address of its choosing as the row draws. The desktop's content policy
// already keeps its webview to such addresses, and the phone has no such
// policy. A block of a reply shows its first pictures, and the rest wait for
// a tap, so a reply that names hundreds can't make a client fetch them all.

import { t } from "../i18n";

import { plural } from "../status/format";

/** How many pictures a block of a reply shows before the rest wait for a tap. */
export const PICTURES_SHOWN = 12;

// A preview as maxd makes one: a JPEG, or a PNG for a picture with
// see-through parts, as `is_preview` in the daemon's thumbnail.rs reads it.
const PREVIEW = /^data:image\/(?:jpeg|png);base64,[A-Za-z0-9+/=]+$/;

/** The longest preview maxd makes, as its `URL_CAP`. */
const PREVIEW_MOST = 48 * 1024;

/**
 * A file's preview as a client may draw it, or null when it has none or has
 * one maxd wouldn't make, such as a web address, so its plain frame shows.
 */
export const previewOf = (file: { readonly thumbnail: string | null }): string | null => {
	const { thumbnail } = file;
	return thumbnail !== null && thumbnail.length <= PREVIEW_MOST && PREVIEW.test(thumbnail)
		? thumbnail
		: null;
};

/**
 * The pictures a block of a reply shows, and how many more wait for a tap.
 * `all` shows every one, once you've asked.
 */
export const shownPictures = <T>(
	pictures: readonly T[],
	all: boolean
): { readonly shown: readonly T[]; readonly more: number } =>
	all || pictures.length <= PICTURES_SHOWN
		? { shown: pictures, more: 0 }
		: { shown: pictures.slice(0, PICTURES_SHOWN), more: pictures.length - PICTURES_SHOWN };

/** The button that shows the rest of a block's pictures. */
export const morePictures = (more: number): string =>
	t("Show {{plural_more___more_picture____more_pictures}}", {
		plural_more___more_picture____more_pictures: plural(more, "more picture", "more pictures"),
	});
