// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the file pane and its viewer frame say to each other. The frame runs
// on an opaque origin, so neither side can name the other's origin, and
// each checks instead that a message came from the window it expects.

import type { Theme } from "@maxhq/shared/theme/theme";

/** The frame is loaded and waits for a file. */
export type FrameReady = { readonly type: "maxhq-viewer:ready" };

/**
 * A file for the frame to show, its bytes moved over rather than copied,
 * and the theme to draw it in.
 */
export type OpenFile = {
	readonly type: "maxhq-viewer:open";
	readonly name: string;
	readonly extension: string;
	readonly bytes: ArrayBuffer;
	readonly theme: Theme;
};

/** How showing the file went. */
export type FrameShown =
	| { readonly type: "maxhq-viewer:shown" }
	| { readonly type: "maxhq-viewer:failed"; readonly message: string };

export type FromFrame = FrameReady | FrameShown;

const FROM_FRAME: ReadonlySet<string> = new Set([
	"maxhq-viewer:ready",
	"maxhq-viewer:shown",
	"maxhq-viewer:failed",
]);

/** Whether a message's data is one the frame sends. */
export const isFromFrame = (data: unknown): data is FromFrame =>
	typeof data === "object" &&
	data !== null &&
	FROM_FRAME.has((data as { type?: unknown }).type as string) &&
	((data as { type: string }).type !== "maxhq-viewer:failed" ||
		typeof (data as { message?: unknown }).message === "string");

/** Whether a message's data is a file to open. */
export const isOpenFile = (data: unknown): data is OpenFile => {
	if (typeof data !== "object" || data === null) return false;
	const message = data as Partial<OpenFile>;
	return (
		message.type === "maxhq-viewer:open" &&
		typeof message.name === "string" &&
		typeof message.extension === "string" &&
		message.bytes instanceof ArrayBuffer &&
		(message.theme === "light" || message.theme === "dark")
	);
};

/**
 * What the frame is allowed to do. Scripts run, and nothing else: without
 * allow-same-origin its origin is opaque, so it can't reach the app's
 * window, storage or backend.
 */
export const FRAME_SANDBOX = "allow-scripts";
