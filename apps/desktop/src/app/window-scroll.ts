// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Keeps the window itself from scrolling. Only what's inside a pane or the
// sidebar scrolls, and the body hides its overflow, but hidden overflow can
// still be scrolled by code. A popup is placed absolutely in the body, so
// one that reaches past the window's bottom makes room to scroll, and a
// focus or a caret kept in view then slid the whole app up under the title
// bar and left an empty band below the status bar, with no way to scroll
// it back. Clipping the body would stop it too, but it would also blank the
// drag image of a tab, which is drawn far above the window. So the window
// goes back to its corner whenever it moves, before it draws again.

/** Puts the window back at its top left corner if it scrolled. */
export const unscrollWindow = (): void => {
	if (window.scrollX !== 0 || window.scrollY !== 0) window.scrollTo(0, 0);
};

/** Starts keeping the window still. Returns the call that stops it. */
export const watchWindowScroll = (): (() => void) => {
	window.addEventListener("scroll", unscrollWindow, { passive: true });
	return () => window.removeEventListener("scroll", unscrollWindow);
};
