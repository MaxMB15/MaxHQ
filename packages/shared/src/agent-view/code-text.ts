// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A file the viewer draws as code or text: whether its bytes are text at
// all, its lines, and whether it's short enough to take colours. The desktop
// and the phone read a file's bytes their own way and share these.

/** How far into a file a zero byte marks it as binary. */
export const BINARY_SNIFF = 8 * 1024;

/** A file larger than this shows without colours. */
export const HIGHLIGHT_BYTES = 512 * 1024;

/** A file with more lines than this shows without colours. */
export const HIGHLIGHT_LINES = 10_000;

/** Whether a file whose first bytes these are is binary: a zero byte comes in its first 8 KB. */
export const looksBinary = (bytes: Uint8Array): boolean =>
	bytes.subarray(0, BINARY_SNIFF).includes(0);

/** A file's text as lines. A last line break doesn't make an empty line. */
export const textLines = (text: string): string[] => {
	const lines = text.split(/\r\n|\r|\n/);
	if (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
	return lines;
};

/** Whether a file this long takes colours. */
export const colourable = (read: { lines: readonly string[]; bytes: number }): boolean =>
	read.bytes <= HIGHLIGHT_BYTES && read.lines.length <= HIGHLIGHT_LINES;
