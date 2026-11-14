// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How tall each visual drew, by its page, so a visual scrolled back to or
// drawn again starts at its height and the feed doesn't jump. The most
// recent ones are kept, up to a limit.

import { visualsIn } from "@maxhq/shared/visuals/fence";
import { visualKey } from "@maxhq/shared/visuals/visual";

const KEPT = 500;

/** A visual's card around its frame: the title row and the borders. */
export const CARD_CHROME_PX = 36;

/** A frame's height before its page says how tall it is. */
export const FIRST_HEIGHT_PX = 160;

const heights = new Map<string, number>();

/** How tall the page with this key last drew, if it's known. */
export const knownHeight = (key: string): number | undefined => heights.get(key);

/** Notes how tall the page with this key drew. */
export const noteHeight = (key: string, height: number): void => {
	heights.delete(key);
	heights.set(key, height);
	if (heights.size > KEPT) {
		const oldest = heights.keys().next().value;
		if (oldest !== undefined) heights.delete(oldest);
	}
};

/** About how much the visuals in a Markdown text add to its height. */
export const visualsHeight = (markdown: string): number =>
	visualsIn(markdown).reduce(
		(sum, { html }) => sum + CARD_CHROME_PX + (knownHeight(visualKey(html)) ?? FIRST_HEIGHT_PX),
		0
	);

/** How tall a replaced visual's folded line is. */
export const FOLDED_PX = 28;

/**
 * About how tall a visual the agent showed draws: folded when it's been
 * replaced, else its card at its page's last height once the page is read.
 */
export const visualItemHeight = (page: string | undefined, folded: boolean): number =>
	folded
		? FOLDED_PX
		: CARD_CHROME_PX +
			((page === undefined ? undefined : knownHeight(visualKey(page))) ?? FIRST_HEIGHT_PX);
