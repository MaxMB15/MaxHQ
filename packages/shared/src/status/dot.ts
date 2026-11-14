// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The shape of each dot from decisions N-1 and N-2, for every app to draw
// its own way. Every state has its own shape, so none relies on color alone.

import type { ActivityLook } from "./activity";
import type { ConnectionLook } from "./connection";

/** The status color a dot is drawn in. */
export type DotTone =
	"busy" | "ready" | "warm" | "linked" | "connecting" | "error" | "off" | "muted";

/** A mark cut out of a filled dot. */
export type DotGlyph = "clock" | "pause" | "cross";

/** How a dot is drawn. */
export type DotShape =
	/** A filled circle. */
	| { readonly kind: "fill"; readonly tone: DotTone }
	/** A turning arc on a faint ring. Starting turns slower than busy. */
	| { readonly kind: "spin"; readonly tone: DotTone; readonly slow: boolean }
	/** A filled circle with a small light badge at its top right. */
	| { readonly kind: "badge"; readonly tone: DotTone }
	/** An empty ring, dashed or solid. */
	| { readonly kind: "ring"; readonly tone: DotTone; readonly dashed: boolean }
	/** A thick ring with a small dot of another color inside. */
	| { readonly kind: "ring_dot"; readonly tone: DotTone; readonly dot: DotTone }
	/** A smaller filled circle inside a ring of the same color. */
	| { readonly kind: "halo"; readonly tone: DotTone }
	/**
	 * A smaller filled circle inside a dashed ring that turns slowly: done,
	 * with work still going on around it. It reads the same standing still.
	 */
	| { readonly kind: "orbit"; readonly tone: DotTone; readonly dot: DotTone }
	/** A filled circle with a mark cut out. */
	| { readonly kind: "glyph"; readonly tone: DotTone; readonly glyph: DotGlyph }
	/** A ring with its left half filled. */
	| { readonly kind: "half"; readonly tone: DotTone }
	/** A ring crossed by a slash. */
	| { readonly kind: "slash"; readonly tone: DotTone };

/** The activity dot for each look. */
export const ACTIVITY_DOT: Record<ActivityLook, DotShape> = {
	starting: { kind: "spin", tone: "muted", slow: true },
	busy: { kind: "spin", tone: "busy", slow: false },
	busy_in_subagents: { kind: "ring_dot", tone: "busy", dot: "ready" },
	needs_input: { kind: "badge", tone: "ready" },
	done_unread: { kind: "halo", tone: "ready" },
	idle_cache_warm: { kind: "fill", tone: "warm" },
	idle: { kind: "fill", tone: "ready" },
	idle_running: { kind: "orbit", tone: "ready", dot: "ready" },
	idle_waiting: { kind: "orbit", tone: "busy", dot: "ready" },
	waiting_for_limit_reset: { kind: "glyph", tone: "busy", glyph: "clock" },
	paused: { kind: "glyph", tone: "off", glyph: "pause" },
	error: { kind: "glyph", tone: "off", glyph: "cross" },
	not_accessible: { kind: "ring", tone: "off", dashed: false },
};

/** The connection dot for each look. */
export const CONNECTION_DOT: Record<ConnectionLook, DotShape> = {
	live: { kind: "fill", tone: "ready" },
	asleep: { kind: "ring", tone: "ready", dashed: false },
	connecting: { kind: "spin", tone: "connecting", slow: false },
	agent_down: { kind: "half", tone: "linked" },
	unreachable: { kind: "slash", tone: "error" },
	unknown: { kind: "ring", tone: "off", dashed: true },
};
