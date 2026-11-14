// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The pages of the visuals open in tabs, by their key. A pane's params go
// into the saved layout, and a page can be 512 KB, so the pane keeps only
// the key and the page stays here while the window runs. A pane restored
// after a restart finds nothing here and closes.

import type { VisualTransfer } from "@maxhq/shared/visuals/transfer";
import { visualKey } from "@maxhq/shared/visuals/visual";

const pages = new Map<string, VisualTransfer>();

/** Keeps a visual's page for its tab, and returns its key. */
export const keepVisualPage = (visual: VisualTransfer): string => {
	const key = visualKey(visual.html);
	pages.set(key, visual);
	return key;
};

/** The page kept under `key`, if this window has it. */
export const visualPage = (key: string): VisualTransfer | undefined => pages.get(key);

/** Forgets every page, for tests. */
export const clearVisualPages = (): void => pages.clear();
