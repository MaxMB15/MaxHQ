// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How a row of controls stays on one line as its pane narrows: it folds a
// step at a time, and each level is one more step folded. The agent's
// header states how wide each level is, and the composer's footer
// measures it. Either way, a row folds as soon as it runs out of room but
// unfolds only once the wider level fits with some room to spare, so a
// pane resized near a boundary doesn't flicker between two levels.

/** How many pixels a wider level must leave free before the row takes it again. */
export const FOLD_SLACK = 8;

/**
 * The level a row `width` pixels wide shows, from the one it shows now.
 * `needs[level]` is how wide the row is at that level, with level 0
 * folding nothing, and undefined where it isn't known yet: a level the
 * row showed without running out of room, or one it hasn't tried. The row
 * folds while its level is known not to fit and unfolds while the wider
 * level fits with `slack` to spare. It unfolds to a level it hasn't tried
 * one step at a time, so it can measure that level before going further.
 */
export const fitLevel = (
	width: number,
	needs: readonly (number | undefined)[],
	level = 0,
	slack = FOLD_SLACK
): number => {
	const last = needs.length - 1;
	let at = Math.max(0, Math.min(level, last));
	while (at < last && (needs[at] ?? 0) > width) at += 1;
	while (at > 0) {
		const need = needs[at - 1];
		if (need !== undefined && need > width - slack) break;
		at -= 1;
		if (need === undefined) break;
	}
	return at;
};

/** The steps a row has folded at `level`, from the order they fold in. */
export const foldsAt = <Step>(order: readonly Step[], level: number): ReadonlySet<Step> =>
	new Set(order.slice(0, level));
