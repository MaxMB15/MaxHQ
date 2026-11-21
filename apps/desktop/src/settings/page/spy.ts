// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Which group of the Settings page you're in, for the left column to mark
// as you scroll: the last group whose heading has reached a line near the
// top, or the last group once the page can't scroll further, since a
// short last group never reaches that line.

/** How far below the top of the scroll box a heading counts as reached, in pixels. */
export const SPY_LINE = 60;

/** How long after a jump the left column waits before following the scroll again. */
export const JUMP_MS = 700;

/** Where the scroll box is. */
export type Scrolled = {
	readonly top: number;
	readonly height: number;
	readonly scrollHeight: number;
};

/**
 * The index of the group you're in, from where each group's top sits below
 * the scroll box's top, in page order. -1 with no groups.
 */
export const currentGroup = (offsets: readonly number[], scrolled: Scrolled): number => {
	if (offsets.length === 0) return -1;
	if (scrolled.top > 0 && scrolled.top + scrolled.height >= scrolled.scrollHeight - 4)
		return offsets.length - 1;
	let current = 0;
	offsets.forEach((offset, at) => {
		if (offset <= SPY_LINE) current = at;
	});
	return current;
};
