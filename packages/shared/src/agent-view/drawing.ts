// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What drawing a diagram or a formula is the same for on the desktop and the
// phone: the settings Mermaid and KaTeX start with, which trust nothing in an
// agent's text, the width a diagram draws at, a drawing's size and its
// address as a picture, and the part of an error that says why it failed.

import { DIAGRAM_CSS } from "./diagram-theme";

/**
 * Mermaid sizes a diagram such as a Gantt chart to what holds it, so this is
 * the same width wherever it draws, and the picture scales from there.
 */
export const STAGE_WIDTH_PX = 760;

/** Mermaid's settings, in its strictest mode, with `themeVariables` from `diagramTheme`. */
export const mermaidSettings = (themeVariables: Record<string, unknown>) => ({
	startOnLoad: false,
	securityLevel: "strict" as const,
	suppressErrorRendering: true,
	htmlLabels: false,
	flowchart: { htmlLabels: false },
	theme: "base" as const,
	themeVariables,
	themeCSS: DIAGRAM_CSS,
	// What a diagram's own `%%{init}%%` can't change.
	secure: [
		"secure",
		"securityLevel",
		"startOnLoad",
		"maxTextSize",
		"maxEdges",
		"suppressErrorRendering",
		"theme",
		"themeCSS",
		"fontFamily",
		"altFontFamily",
		"htmlLabels",
	],
});

// The most, in ems, that a formula's own sizes can reach.
const LARGEST_EM = 20;

/**
 * KaTeX's settings for a formula. A formula can't name a link or a picture,
 * and one it can't read shows in `errorColor`.
 */
export const mathSettings = (display: boolean, output: "html" | "mathml", errorColor: string) => ({
	displayMode: display,
	output,
	throwOnError: false,
	errorColor,
	trust: false,
	strict: "ignore" as const,
	maxSize: LARGEST_EM,
});

/** What Mermaid gave a drawing's root to size it by. */
export type DrawnRoot = {
	/** Its `max-width` style, as `512px`, or empty. */
	readonly maxWidth: string;
	/** Its `width` attribute, which may be a percentage. */
	readonly width: string | null;
	readonly viewBox: string | null;
};

/** A drawing's size at 1:1 in CSS pixels, or null when it has none. */
export const drawingSize = (root: DrawnRoot): { width: number; height: number } | null => {
	const [, , boxWidth = 0, boxHeight = 0] = (root.viewBox ?? "").split(/[\s,]+/).map(Number);
	const attribute = root.width ?? "";
	const wide =
		Number.parseFloat(root.maxWidth) ||
		(attribute.endsWith("%") ? 0 : Number.parseFloat(attribute)) ||
		boxWidth;
	if (!wide || !boxWidth || !boxHeight) return null;
	return { width: Math.ceil(wide), height: Math.ceil((wide * boxHeight) / boxWidth) };
};

/** A cleaned drawing's SVG, with no size of its own, as a picture's address at this size. */
export const svgUrl = (svg: string, width: number, height: number): string =>
	`data:image/svg+xml;charset=utf-8,${encodeURIComponent(
		svg.replace(/^<svg /, `<svg width="${width}" height="${height}" `)
	)}`;

const REASON_LENGTH = 160;

/** Why a drawing failed, as the first line of its error, cut short. */
export const failureReason = (error: unknown): string => {
	const message = error instanceof Error ? error.message : String(error);
	return (message.split("\n")[0] ?? "").slice(0, REASON_LENGTH);
};
