// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Zooming and panning a picture in a frame, as Preview does: it opens
// fitted, never larger than its own pixels, zooms around a point, and pans
// only while it's bigger than the frame. A side smaller than the frame
// stays centred, and a bigger one always covers the frame, so the picture
// can't be panned out of view. Pure numbers, for any view to draw.

/** A width and a height, in points. */
export type Size = { readonly width: number; readonly height: number };

/** A point in the frame, from its top left corner. */
export type Point = { readonly x: number; readonly y: number };

/** How the picture sits: its scale, and where its top left corner is in the frame. */
export type Zoom = { readonly scale: number } & Point;

/** The smallest scale, unless the fitted one is smaller still. */
export const ZOOM_MIN = 0.1;
/** The largest scale, unless the fitted one is larger still. */
export const ZOOM_MAX = 8;
/** How much one step in or out scales by. */
export const ZOOM_STEP = 1.25;

/** The scale that shows the whole picture, at most its own pixels. */
export const fitScale = (picture: Size, frame: Size): number =>
	picture.width > 0 && picture.height > 0
		? Math.min(1, frame.width / picture.width, frame.height / picture.height)
		: 1;

/** The scale `scale` becomes once kept within the bounds for this fit. */
export const clampScale = (scale: number, fit: number): number =>
	Math.min(Math.max(fit, ZOOM_MAX), Math.max(Math.min(fit, ZOOM_MIN), scale));

// Where one side of the picture goes: centred while it's smaller than the
// frame, else held so it covers the frame.
const placeSide = (at: number, size: number, frame: number): number =>
	size <= frame ? (frame - size) / 2 : Math.min(0, Math.max(frame - size, at));

/** `zoom` with its scale within bounds and the picture kept in view. */
export const placed = (zoom: Zoom, picture: Size, frame: Size): Zoom => {
	const scale = clampScale(zoom.scale, fitScale(picture, frame));
	return {
		scale,
		x: placeSide(zoom.x, picture.width * scale, frame.width),
		y: placeSide(zoom.y, picture.height * scale, frame.height),
	};
};

/** The picture fitted to the frame and centred. */
export const fitted = (picture: Size, frame: Size): Zoom =>
	placed({ scale: fitScale(picture, frame), x: 0, y: 0 }, picture, frame);

/** `zoom` scaled to `scale`, keeping the part of the picture under `at` where it is. */
export const zoomAt = (zoom: Zoom, scale: number, at: Point, picture: Size, frame: Size): Zoom => {
	const next = clampScale(scale, fitScale(picture, frame));
	const ratio = next / zoom.scale;
	return placed(
		{ scale: next, x: at.x - (at.x - zoom.x) * ratio, y: at.y - (at.y - zoom.y) * ratio },
		picture,
		frame
	);
};

/** `zoom` moved by `dx` and `dy`, as far as the picture may go. */
export const panBy = (zoom: Zoom, dx: number, dy: number, picture: Size, frame: Size): Zoom =>
	placed({ scale: zoom.scale, x: zoom.x + dx, y: zoom.y + dy }, picture, frame);

/** Whether the picture is bigger than the frame either way, so it pans. */
export const pannable = (zoom: Zoom, picture: Size, frame: Size): boolean =>
	picture.width * zoom.scale > frame.width + 0.5 ||
	picture.height * zoom.scale > frame.height + 0.5;

/** Whether the picture shows at its fitted scale. */
export const isFitted = (zoom: Zoom, picture: Size, frame: Size): boolean =>
	Math.abs(zoom.scale - fitScale(picture, frame)) < 1e-3;

/**
 * What a double click does: from fitted, the picture's own pixels around
 * the click, or twice them when that's what fits; from anything else, fitted.
 */
export const toggleActual = (zoom: Zoom, at: Point, picture: Size, frame: Size): Zoom => {
	if (!isFitted(zoom, picture, frame)) return fitted(picture, frame);
	const fit = fitScale(picture, frame);
	return zoomAt(zoom, fit < 1 ? 1 : 2, at, picture, frame);
};

/** What a zoom key or control does. */
export type ZoomAction = "in" | "out" | "fit" | "actual";

/** What a key does to the picture, as in Preview, or null for a key it leaves alone. */
export const zoomKey = (key: string): ZoomAction | null => {
	switch (key) {
		case "+":
		case "=":
			return "in";
		case "-":
		case "_":
			return "out";
		case "0":
			return "fit";
		case "1":
			return "actual";
		default:
			return null;
	}
};

/**
 * How much a wheel's movement scales by, for a pinch or a ⌘-scroll. A
 * mouse's line steps count as 16 points, and each event moves at most
 * about 1.6 times either way, so a fast wheel doesn't jump.
 */
export const wheelFactor = (deltaY: number, lines: boolean): number => {
	const points = Math.max(-50, Math.min(50, lines ? deltaY * 16 : deltaY));
	return Math.exp(-points * 0.01);
};
