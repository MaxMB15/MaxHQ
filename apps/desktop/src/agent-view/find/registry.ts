// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The agent views, and the terminals in them, a find shortcut can open
// find in. Each says where it sits in the page, and the shortcut picks the
// innermost one focus is in, then the outermost one in the active pane.

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
 * The call that opens find for the find shortcut: in the innermost view
 * that holds focus, such as a terminal in an agent view, else the
 * outermost one in the active pane, else the only outermost one. Null
 * when there's no such view.
 */
export const finderFor = (focus: Element | null, pane: Element | null): (() => void) | null => {
	const all = [...finders];
	const inside = (outer: Finder, inner: Finder) =>
		outer !== inner && outer.element.contains(inner.element);
	const holding = focus ? all.filter((finder) => finder.element.contains(focus)) : [];
	const outermost = all.filter((finder) => !all.some((other) => inside(other, finder)));
	const found =
		holding.find((finder) => !holding.some((other) => inside(finder, other))) ||
		(pane && outermost.find((finder) => pane.contains(finder.element))) ||
		(outermost.length === 1 ? outermost[0] : undefined);
	return found?.open ?? null;
};
