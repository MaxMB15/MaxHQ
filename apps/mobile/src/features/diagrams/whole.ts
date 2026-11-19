// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How a diagram shown whole is laid out and zoomed. A pinch scales the
// drawing's picture as it was laid out, so it's laid out at its own size,
// where its text reads as drawn and stays sharp up to there, and starts
// zoomed out to fit the screen. A small one is laid out to fit instead. A
// very large one is laid out smaller, so its picture doesn't take too much
// memory.

/** The longest side, in points, a drawing is laid out at. */
export const LARGEST = 2048;

/** How far a pinch goes past fitting the screen. */
export const MOST_ZOOM = 5;

type Size = { readonly width: number; readonly height: number };

/** A drawing laid out whole: its size, and the zooms that fit it, the most a pinch goes and where it starts. */
export type WholeLayout = Size & { readonly least: number; readonly most: number };

/** Lays out a drawing of `drawing`'s size for a view of `box`'s size. */
export const wholeLayout = (drawing: Size, box: Size): WholeLayout => {
	const fit = Math.min(box.width / drawing.width, box.height / drawing.height);
	const sharp = Math.min(1, LARGEST / Math.max(drawing.width, drawing.height));
	const laid = Math.max(fit, sharp);
	return {
		width: Math.round(drawing.width * laid),
		height: Math.round(drawing.height * laid),
		least: fit / laid,
		most: Math.max(1, (MOST_ZOOM * fit) / laid),
	};
};
