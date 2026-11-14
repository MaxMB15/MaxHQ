// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The box you ask the dispatcher in, which on the desktop is also the box
// you answer an agent in. While you're elsewhere it keeps to one line and
// cuts the draft off. While you type in it, it opens to the whole draft,
// wrapped, and grows a line at a time up to a few lines, then scrolls.
// Each client draws it its own way from these.

/** How many lines the box grows to before it scrolls, per client. */
export const FIELD_LINES = { desktop: 8, phone: 5 } as const;

/** The tallest the box grows: `lines` lines of `lineHeight`, with `padding` above and below. */
export const fieldMaxHeight = (lines: number, lineHeight: number, padding: number): number =>
	lines * lineHeight + padding * 2;

/** What Enter does in the box. */
export type EnterAction = "send" | "confirm" | "none";

/** Enter sends the draft, or with the box blank confirms the plan shown, if there is one. */
export const enterAction = (text: string, hasPlan: boolean): EnterAction =>
	text.trim() !== "" ? "send" : hasPlan ? "confirm" : "none";
