// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Keeps the shown tab in sight in its group's tab strip. dockview scrolls
// the strip to a tab as it becomes active, but a new tab draws its title a
// moment later. The strip can then stop short of it, and the tab ends up
// in the overflow list instead of the strip.

/**
 * Scrolls the tab strip that holds `inside` just far enough to show its
 * whole tab. It does nothing when the tab already shows, or when `inside`
 * sits in the overflow list rather than the strip.
 */
export const revealTab = (inside: HTMLElement): void => {
	const tab = inside.closest<HTMLElement>(".dv-tabs-container > .dv-tab");
	const strip = tab?.parentElement;
	if (!tab || !strip) return;
	const start = tab.offsetLeft;
	const end = start + tab.offsetWidth;
	const from = strip.scrollLeft;
	const width = strip.clientWidth;
	const to = start < from ? start : end > from + width ? Math.min(start, end - width) : from;
	if (to === from) return;
	strip.scrollLeft = to;
	// dockview's scrollbar and its overflow list update on this event.
	strip.dispatchEvent(new Event("scroll"));
};
