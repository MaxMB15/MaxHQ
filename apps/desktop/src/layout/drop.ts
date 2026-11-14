// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Where an agent dragged in from the sidebar would land. Under the pointer
// strategy dockview 8.6.1 turns HTML5 drags off and runs its own drags
// through a controller it keeps to itself, so nothing from outside can reach
// its drop targets. This reads the groups' boxes the way those targets do:
// the outer fifth of a group splits from that side, the rest of it and its
// tab bar add a tab, a floating group only takes tabs, and the outer few
// pixels of the grid split from the whole grid.

import type { DockviewApi, DockviewGroupPanel } from "dockview-react";

import { shownRect } from "./commands";
import type { DropSide, DropSpot } from "./panes";

/** A box on screen. */
export type Box = {
	readonly left: number;
	readonly top: number;
	readonly width: number;
	readonly height: number;
};

/** A spot to drop on and the box that shows where the pane would go. */
export type DropTarget = { readonly spot: DropSpot; readonly box: Box };

// How much of a group from each edge splits it, as a fraction.
const EDGE = 0.2;
// How near the grid's edge splits the whole grid, and how much of it lights
// up then, in pixels.
const GRID_EDGE = 10;
const GRID_BAND = 20;

const boxOf = (rect: DOMRect): Box => ({
	left: rect.left,
	top: rect.top,
	width: rect.width,
	height: rect.height,
});

const inside = (box: Box, x: number, y: number): boolean =>
	x >= box.left && x < box.left + box.width && y >= box.top && y < box.top + box.height;

/** The side of `box` a point falls in, or its middle. */
export const sideAt = (box: Box, x: number, y: number): DropSide => {
	const across = (x - box.left) / box.width;
	const down = (y - box.top) / box.height;
	if (across < EDGE) return "left";
	if (across > 1 - EDGE) return "right";
	if (down < EDGE) return "top";
	if (down > 1 - EDGE) return "bottom";
	return "center";
};

// The part of `box` a drop on `side` fills: `band` pixels on that side, or
// half of it when there's no band.
const part = (box: Box, side: DropSide, band?: number): Box => {
	const width = band ?? box.width / 2;
	const height = band ?? box.height / 2;
	switch (side) {
		case "left":
			return { ...box, width };
		case "right":
			return { ...box, left: box.left + box.width - width, width };
		case "top":
			return { ...box, height };
		case "bottom":
			return { ...box, top: box.top + box.height - height, height };
		case "center":
			return box;
	}
};

const gridSide = (area: Box, x: number, y: number): DropSide | null => {
	if (x - area.left < GRID_EDGE) return "left";
	if (area.left + area.width - x <= GRID_EDGE) return "right";
	if (y - area.top < GRID_EDGE) return "top";
	if (area.top + area.height - y <= GRID_EDGE) return "bottom";
	return null;
};

// A group's box under its tab bar, the part a drop can split.
const contentOf = (group: DockviewGroupPanel, box: Box): Box => {
	const bar = group.element.querySelector(".dv-tabs-and-actions-container");
	const height = bar?.getBoundingClientRect().height ?? 0;
	return { ...box, top: box.top + height, height: box.height - height };
};

/**
 * Where a drop at a point on screen would land in the grid that `root`
 * holds, or null when it would land nowhere, as in the gap between groups.
 */
export const targetAt = (
	api: DockviewApi,
	root: HTMLElement,
	x: number,
	y: number
): DropTarget | null => {
	const area = boxOf(root.getBoundingClientRect());
	if (!inside(area, x, y)) return null;
	const shown = api.groups.flatMap((group) => {
		const rect = shownRect(group);
		return rect ? [{ group, box: boxOf(rect) }] : [];
	});
	// Floating groups sit above the grid.
	const floating = shown.find(
		({ group, box }) => group.api.location.type === "floating" && inside(box, x, y)
	);
	if (floating) return { spot: { group: floating.group.id, side: "center" }, box: floating.box };
	const docked = shown.filter(({ group }) => group.api.location.type === "grid");
	if (docked.length === 0) return { spot: { group: null, side: "center" }, box: area };
	const edge = gridSide(area, x, y);
	if (edge) return { spot: { group: null, side: edge }, box: part(area, edge, GRID_BAND) };
	const hit = docked.find(({ box }) => inside(box, x, y));
	if (!hit) return null;
	const content = contentOf(hit.group, hit.box);
	const side = inside(content, x, y) ? sideAt(content, x, y) : "center";
	return { spot: { group: hit.group.id, side }, box: part(content, side) };
};
