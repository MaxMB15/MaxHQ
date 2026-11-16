// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How tall a lease's screen picture draws: as wide as its box, as tall as
// its shape asks, and never taller than most of a phone's screen.

/** The tallest a lease's screen draws, in points. */
export const SCREEN_TALLEST = 480;

/** How tall it draws before the picture has come. */
export const SCREEN_WAITING = 220;

/** The picture's height for a box `width` wide, given the picture's width over its height. */
export const screenHeight = (width: number, ratio: number | null): number =>
	width > 0 && ratio !== null && ratio > 0
		? Math.min(SCREEN_TALLEST, Math.round(width / ratio))
		: SCREEN_WAITING;
