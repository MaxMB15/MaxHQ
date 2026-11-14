// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Where the file pane gets what it shows: the viewer frame's page, and a
// file's bytes from the media cache. The cache sends a large file a chunk
// at a time, so reading one asks for each next range until it has them all.

import { t } from "@maxhq/shared/i18n";

import { convertFileSrc, isTauri } from "@tauri-apps/api/core";

/**
 * A frame's page, such as `viewer.html`. The built app serves it through its
 * own scheme, and the dev server and the browser preview serve it beside the
 * app.
 */
export const framePageUrl = (page: string, dev = import.meta.env.DEV, tauri = isTauri()): string =>
	!dev && tauri ? convertFileSrc(page, "maxview") : `/${page}`;

/** The viewer frame's page. */
export const viewerFrameUrl = (dev = import.meta.env.DEV, tauri = isTauri()): string =>
	framePageUrl("viewer.html", dev, tauri);

// How long the file is, from a `Content-Range` such as "bytes 0-99/1000".
const totalOf = (response: Response): number | null => {
	const total = /\/(\d+)\s*$/.exec(response.headers.get("Content-Range") ?? "")?.[1];
	return total === undefined ? null : Number(total);
};

const failed = (response: Response) =>
	new Error(`The file didn't come through (HTTP ${response.status}).`);

/** Reads every byte of the cached file at `url`, `size` bytes as the cache said. */
export const readBytes = async (
	url: string,
	size: number,
	get: typeof fetch = fetch
): Promise<ArrayBuffer> => {
	let response = await get(url);
	if (!response.ok) throw failed(response);
	if (response.status !== 206) return response.arrayBuffer();
	const total = totalOf(response) ?? size;
	const bytes = new Uint8Array(total);
	let at = 0;
	for (;;) {
		const chunk = new Uint8Array(await response.arrayBuffer());
		if (chunk.byteLength === 0 || at + chunk.byteLength > total) {
			throw new Error(t("The file changed while it was read."));
		}
		bytes.set(chunk, at);
		at += chunk.byteLength;
		if (at === total) return bytes.buffer;
		response = await get(url, { headers: { Range: `bytes=${at}-` } });
		if (response.status !== 206) throw failed(response);
	}
};
