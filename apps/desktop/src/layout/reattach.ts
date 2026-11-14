// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// dockview takes a pane's content out of the page and puts it straight back
// when the pane opens again in its own group, as a click on its tab does
// even when the tab already shows. The browser scrolls everything inside
// back to the top and sends no scroll or resize event, so a list that keeps
// its own scroll position draws rows that aren't in view, and the pane
// stays blank until the next scroll. This tells such a list when its
// pane's content is back in the page.

/** The element dockview puts a pane's content in. */
const PART = ".dv-react-part";

/**
 * Calls `back` before the next paint once the pane content around `el` is
 * put back in the page, once a frame however often that happens. Returns a
 * function that stops watching. Outside a pane it does nothing.
 */
export const onReattach = (el: Element, back: () => void): (() => void) => {
	const part = el.closest(PART);
	if (!part) return () => {};
	let frame = 0;
	let watched: Node | null = null;
	const observer = new MutationObserver((records) => {
		const touched = records.some(
			(r) => [...r.addedNodes].includes(part) || [...r.removedNodes].includes(part)
		);
		// Out of the page for now: it comes back here, or a resize says so.
		if (!touched || !part.isConnected) return;
		watch();
		if (frame) return;
		frame = requestAnimationFrame(() => {
			frame = 0;
			back();
		});
	});
	// A pane that moves to another group comes back in a new parent.
	const watch = () => {
		const parent = part.parentNode;
		if (!parent || parent === watched) return;
		observer.disconnect();
		watched = parent;
		observer.observe(parent, { childList: true });
	};
	watch();
	return () => {
		observer.disconnect();
		if (frame) cancelAnimationFrame(frame);
	};
};
