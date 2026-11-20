// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How tall a visual's card draws in a feed or a note: as tall as its page,
// up to most of the screen, past which the page scrolls inside and Open
// shows it whole. Each page's height is kept, so a card scrolled back to,
// or run again, starts at it and the feed doesn't jump.

import { keep, keptValue } from "@maxhq/shared/agent-view/kept";
import { visualKey } from "@maxhq/shared/visuals/visual";

import type { VisualToOpen } from "./visual";

/** A card's page height before the page says how tall it is, as the desktop's. */
export const FIRST_HEIGHT = 160;

// The share of the screen a card's page takes at most.
const MOST_OF_SCREEN = 0.7;

/** The tallest a card's page draws on a screen `screen` tall. */
export const cardCap = (screen: number): number =>
	Math.max(FIRST_HEIGHT, Math.round(screen * MOST_OF_SCREEN));

/** How tall a page `page` tall draws in a card, and whether it scrolls inside. */
export const cardHeight = (
	page: number,
	cap: number
): { readonly height: number; readonly scrolls: boolean } => ({
	height: Math.min(page, cap),
	scrolls: page > cap,
});

/** The key a visual's height is kept under: its page, or its file on the agent's device. */
export const heightKey = (visual: VisualToOpen): string =>
	visual.kind === "page"
		? `visual-height\n${visualKey(visual.html)}`
		: `visual-height\n${visual.agent}\n${visual.path}`;

/** How tall the visual with this key last drew, or FIRST_HEIGHT. */
export const knownHeight = (key: string): number => keptValue(key, FIRST_HEIGHT);

/** Keeps how tall the visual with this key drew. */
export const keepHeight = (key: string, height: number): void => keep(key, height);

/** Whether two visuals are the same one, though they're new objects. */
export const sameVisual = (a: VisualToOpen, b: VisualToOpen): boolean =>
	a === b ||
	(a.title === b.title &&
		(a.kind === "page"
			? b.kind === "page" && a.html === b.html
			: b.kind === "device" && a.agent === b.agent && a.path === b.path));
