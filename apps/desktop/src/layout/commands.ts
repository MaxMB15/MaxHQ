// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the pane buttons and shortcuts do to a window's panes. dockview's free
// tier moves focus only in a fixed order, so moving by direction reads where
// each group sits on screen. Each returns whether it did anything.

import type { DockviewApi, DockviewGroupPanel, IDockviewPanel } from "dockview-react";

/** A way to move focus from one group to the next. */
export type FocusDirection = "left" | "right" | "up" | "down";

/** Where a split puts the new group. */
export type SplitDirection = "right" | "below";

// Groups sit a few pixels apart, so an edge this close to another still
// counts as touching it.
const SLACK = 8;

/** Where a group sits on screen, or null while it's hidden. */
export const shownRect = (group: DockviewGroupPanel): DOMRect | null => {
	const rect = group.element.getBoundingClientRect();
	return rect.width > 0 && rect.height > 0 ? rect : null;
};

/**
 * Focuses an open pane. One that already shows in its group only moves the
 * focus to that group: dockview opens the pane again otherwise, which takes
 * its content out of the page and puts it back, and a list inside scrolls
 * to the top without knowing it, so the pane goes blank until a scroll.
 */
export const focusPane = (pane: IDockviewPanel): void => {
	if (pane.group.activePanel === pane) pane.group.api.setActive();
	else pane.api.setActive();
};

const activate = (group: DockviewGroupPanel): void => {
	const panel = group.activePanel ?? group.panels[0];
	if (panel) focusPane(panel);
	else group.api.setActive();
};

type Candidate = {
	readonly group: DockviewGroupPanel;
	readonly overlaps: boolean;
	readonly gap: number;
	readonly offset: number;
};

// A group that lines up with the focused one beats one that doesn't, then
// the nearer one wins, then the one more in line.
const better = (a: Candidate, b: Candidate): boolean => {
	if (a.overlaps !== b.overlaps) return a.overlaps;
	if (Math.abs(a.gap - b.gap) > 1) return a.gap < b.gap;
	return a.offset < b.offset;
};

/** Focuses the group next to the focused one in `direction`. */
export const focusGroup = (api: DockviewApi, direction: FocusDirection): boolean => {
	const from = api.activeGroup;
	const start = from ? shownRect(from) : null;
	if (!from || !start) return false;
	const across = direction === "left" || direction === "right";
	let best: Candidate | null = null;
	for (const group of api.groups) {
		if (group === from) continue;
		const rect = shownRect(group);
		if (!rect) continue;
		const gap =
			direction === "right"
				? rect.left - start.right
				: direction === "left"
					? start.left - rect.right
					: direction === "down"
						? rect.top - start.bottom
						: start.top - rect.bottom;
		if (gap < -SLACK) continue;
		const overlap = across
			? Math.min(rect.bottom, start.bottom) - Math.max(rect.top, start.top)
			: Math.min(rect.right, start.right) - Math.max(rect.left, start.left);
		const offset = across
			? Math.abs(rect.top + rect.height / 2 - (start.top + start.height / 2))
			: Math.abs(rect.left + rect.width / 2 - (start.left + start.width / 2));
		const candidate = { group, overlaps: overlap > 0, gap, offset };
		if (!best || better(candidate, best)) best = candidate;
	}
	if (!best) return false;
	activate(best.group);
	return true;
};

/** Shows the next tab in the focused group, or the one before, going round. */
export const cycleTab = (api: DockviewApi, step: 1 | -1): boolean => {
	const group = api.activeGroup;
	const panels = group?.panels ?? [];
	if (!group || panels.length < 2) return false;
	const at = group.activePanel ? panels.indexOf(group.activePanel) : -1;
	panels[(at + step + panels.length) % panels.length]!.api.setActive();
	return true;
};

/** Shows the focused group's tab at `place`, counting from 1. Place 9 is always the last tab. */
export const showTab = (api: DockviewApi, place: number): boolean => {
	const panels = api.activeGroup?.panels ?? [];
	const panel = place === 9 ? panels.at(-1) : panels[place - 1];
	if (!panel || panel === api.activePanel) return false;
	panel.api.setActive();
	return true;
};

/** Whether one of a group's tabs can move to a split of its own. */
export const canSplit = (group: DockviewGroupPanel): boolean =>
	group.api.location.type !== "floating" && group.panels.length > 1;

/** Moves a group's visible tab, or `panel`, to a new group beside it. */
export const splitGroup = (
	api: DockviewApi,
	group: DockviewGroupPanel | undefined,
	direction: SplitDirection,
	panel: IDockviewPanel | undefined = group?.activePanel
): boolean => {
	if (!group || !panel || !canSplit(group)) return false;
	if (api.hasMaximizedGroup()) api.exitMaximizedGroup();
	const beside = api.addGroup({ referenceGroup: group, direction });
	panel.api.moveTo({ group: beside });
	return true;
};

/** Maximizes a docked group, or restores it when it already is. */
export const toggleMaximized = (group: DockviewGroupPanel | undefined): boolean => {
	if (!group || group.api.location.type === "floating") return false;
	if (group.api.isMaximized()) group.api.exitMaximized();
	else group.api.maximize();
	return true;
};

/** Closes the focused pane. Its agent keeps running. */
export const closePane = (api: DockviewApi): boolean => {
	const panel = api.activePanel;
	if (!panel) return false;
	panel.api.close();
	return true;
};
