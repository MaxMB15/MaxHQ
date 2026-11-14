// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The agent views a find shortcut can open find in. Each view says where
// it sits in the page, and the shortcut picks the one focus is in, then
// the one in the active pane.

type Finder = { readonly element: Element; readonly open: () => void };

const finders = new Set<Finder>();

/** Lets the find shortcut open find in the agent view at `element`. Returns the undo. */
export const registerFinder = (element: Element, open: () => void): (() => void) => {
	const finder = { element, open };
	finders.add(finder);
	return () => {
		finders.delete(finder);
	};
};

/**
 * The call that opens find for the find shortcut: in the view that holds
 * focus, else the one in the active pane, else the only one. Null when
 * there's no such view.
 */
export const finderFor = (focus: Element | null, pane: Element | null): (() => void) | null => {
	const all = [...finders];
	const found =
		(focus && all.find((finder) => finder.element.contains(focus))) ||
		(pane && all.find((finder) => pane.contains(finder.element))) ||
		(all.length === 1 ? all[0] : undefined);
	return found?.open ?? null;
};
