// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How the composer's bottom row makes room as its pane narrows. It stays
// on one line at any width: its controls give up words a step at a time,
// in a fixed order, and each keeps its full text in its tooltip and its
// accessible name. The row measures how wide it is at each step and picks
// one with `fitLevel`.

/** A step the footer folds. */
export type FooterStep = "size" | "count" | "keys" | "permission" | "effort" | "model" | "send";

// The order they fold in. The attachments' size goes first, then their
// count moves onto the paperclip, as the box above shows each file anyway.
// The key hints go next, as you learn those once. The permission mode and
// the effort fold to their icons, then the model gives up letters, as it
// matters most, and the send buttons fold to their icons last.
const STEPS: readonly FooterStep[] = [
	"size",
	"count",
	"keys",
	"permission",
	"effort",
	"model",
	"send",
];

/** What the footer shows, which decides which steps it has. */
export type FooterShows = {
	/** Whether the attachments' summary gives their size. */
	readonly sized: boolean;
	readonly attachments: boolean;
	readonly effort: boolean;
	readonly permission: boolean;
};

// The steps that fold something only when the footer shows it.
const SHOWN_BY: Partial<Record<FooterStep, keyof FooterShows>> = {
	size: "sized",
	count: "attachments",
	permission: "permission",
	effort: "effort",
};

/**
 * The steps a footer that shows this folds, in order. It leaves out a step
 * with nothing to fold, so every level of the row looks different.
 */
export const footerSteps = (shows: FooterShows): FooterStep[] =>
	STEPS.filter((step) => {
		const shownBy = SHOWN_BY[step];
		return shownBy === undefined || shows[shownBy];
	});
