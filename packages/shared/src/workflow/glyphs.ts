// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How each node status looks. Every status has a shape of its own and a
// label for screen readers, so color never carries the meaning alone. And
// how a workflow's badge tone reads as a badge's.

import { t } from "../i18n";

import type { NodeStatus, Tone } from "../bindings";
import type { Tone as BadgeTone } from "../tasks/format";

/** The shape a status draws. */
export type GlyphShape =
	| "hollow"
	| "ring"
	| "spinner"
	| "half"
	| "person"
	| "check"
	| "slash"
	| "ghost"
	| "cross"
	| "lock"
	| "striped"
	| "faint"
	| "unknown";

/** The color a status draws in, as a theme token. */
export type GlyphColor = "faint" | "quiet" | "muted" | "plain" | "accent" | "warn" | "good" | "bad";

/** How a status moves. Views drop all of it under reduced motion. */
export type GlyphMotion = "none" | "spin" | "pulse" | "fill";

export type Glyph = {
	readonly shape: GlyphShape;
	readonly color: GlyphColor;
	/** Drawn at partial opacity. */
	readonly dim: boolean;
	readonly motion: GlyphMotion;
	/** What a screen reader says, such as "Waiting on you". */
	readonly label: string;
};

const glyph = (
	shape: GlyphShape,
	color: GlyphColor,
	label: string,
	motion: GlyphMotion = "none",
	dim = false
): Glyph => ({
	shape,
	color,
	dim,
	motion,
	get label() {
		return t(label);
	},
});

export const GLYPHS: Readonly<Record<NodeStatus, Glyph>> = {
	omitted: glyph("faint", "faint", "Left out"),
	queued: glyph("hollow", "quiet", "Queued"),
	ready: glyph("ring", "plain", "Ready"),
	busy: glyph("spinner", "accent", "Running", "spin"),
	waiting_child: glyph("half", "accent", "Waiting on another run", "none", true),
	waiting_human: glyph("person", "warn", "Waiting on you", "pulse"),
	done: glyph("check", "good", "Done", "fill"),
	skipped: glyph("slash", "faint", "Skipped"),
	will_skip: glyph("ghost", "faint", "Will be skipped", "none", true),
	failed: glyph("cross", "bad", "Failed"),
	blocked: glyph("lock", "bad", "Blocked", "none", true),
	stale: glyph("striped", "muted", "Out of date"),
	unknown: glyph("unknown", "muted", "Unknown"),
};

/**
 * A node's glyph. A guessed status says so, since the run may still prove
 * it wrong.
 */
export const glyphOf = (status: NodeStatus, provisional = false): Glyph => {
	const base = GLYPHS[status];
	return provisional
		? { ...base, label: t("{{base_label}}, not yet certain", { base_label: base.label }) }
		: base;
};

/** A workflow's tone as a badge's. */
export const badgeTone = (tone: Tone): BadgeTone => (tone === "neutral" ? "plain" : tone);
