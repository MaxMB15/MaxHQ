// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Where a closed pane was, so reopening it puts it back there. Just before
// dockview removes a pane, the layout is read as it still stands: the pane's
// group and its place among the tabs, the box a floating group sat in, and
// a group beside it to split from when its own group closed with it.

import {
	type AnchorPosition,
	type DockviewApi,
	Orientation,
	type SerializedDockview,
	type SerializedGridObject,
} from "dockview-react";

import { type PaneParams, paneParams } from "./pane-id";

type GroupState = NonNullable<NonNullable<SerializedDockview["floatingGroups"]>[number]["data"]>;
type Node = SerializedGridObject<GroupState>;

/** A side of a group that a new group can split onto. */
export type Side = "left" | "right" | "above" | "below";

/** Where a closed pane was. */
export type ClosedPane = {
	/** What it showed. */
	readonly params: PaneParams;
	/** The group it was a tab in. */
	readonly group: string;
	/** Its place among that group's tabs. */
	readonly index: number;
	/** The box its group floated in, when it floated. */
	readonly floating: { position: AnchorPosition; width: number; height: number } | null;
	/** A group it sat beside and the side it sat on, with its size along that side. */
	readonly beside: { group: string | null; side: Side; size: number | undefined } | null;
};

type Leaf = {
	readonly group: GroupState;
	readonly size: number | undefined;
	readonly siblings: readonly Node[];
	readonly at: number;
	readonly orientation: Orientation;
	readonly depth: number;
};

const flip = (o: Orientation): Orientation =>
	o === Orientation.HORIZONTAL ? Orientation.VERTICAL : Orientation.HORIZONTAL;

const childrenOf = (node: Node): readonly Node[] =>
	node.type === "branch" ? (node.data as Node[]) : [];

// The leaf holding the pane `id`, with the branch it sits in.
const findLeaf = (node: Node, orientation: Orientation, id: string, depth = 0): Leaf | null => {
	const siblings = childrenOf(node);
	for (const [at, child] of siblings.entries()) {
		if (child.type === "leaf") {
			const group = child.data as GroupState;
			if (group.views.includes(id)) {
				return { group, size: child.size, siblings, at, orientation, depth };
			}
		} else {
			const found = findLeaf(child, flip(orientation), id, depth + 1);
			if (found) return found;
		}
	}
	return null;
};

// The group in a branch nearest its start or its end.
const edgeLeaf = (node: Node, end: boolean): string | null => {
	if (node.type === "leaf") return (node.data as GroupState).id;
	const children = childrenOf(node);
	const next = end ? children.at(-1) : children[0];
	return next ? edgeLeaf(next, end) : null;
};

// A group beside the leaf to split from, preferring one that shares its
// branch so the split lands exactly where the leaf was. A leaf at either
// end of the top branch splits from the grid's edge instead.
const besideOf = (leaf: Leaf): ClosedPane["beside"] => {
	const along = leaf.orientation === Orientation.HORIZONTAL;
	const after: Side = along ? "right" : "below";
	const before: Side = along ? "left" : "above";
	const prev = leaf.siblings[leaf.at - 1];
	const next = leaf.siblings[leaf.at + 1];
	const size = leaf.size;
	if (prev?.type === "leaf") return { group: (prev.data as GroupState).id, side: after, size };
	if (next?.type === "leaf") return { group: (next.data as GroupState).id, side: before, size };
	if (leaf.depth === 0 && prev && !next) return { group: null, side: after, size };
	if (leaf.depth === 0 && next && !prev) return { group: null, side: before, size };
	if (prev) return { group: edgeLeaf(prev, true), side: after, size };
	if (next) return { group: edgeLeaf(next, false), side: before, size };
	return null;
};

/** Where the pane `id` sits in a layout, or null when it isn't there. */
export const placeOf = (
	layout: SerializedDockview,
	id: string
): Omit<ClosedPane, "params"> | null => {
	for (const floating of layout.floatingGroups ?? []) {
		const group =
			floating.data && floating.data.views.includes(id)
				? floating.data
				: floating.grid
					? findLeaf(floating.grid.root, floating.grid.orientation, id)?.group
					: undefined;
		if (!group) continue;
		const { width, height, ...position } = floating.position;
		return {
			group: group.id,
			index: group.views.indexOf(id),
			floating: { position: position as AnchorPosition, width, height },
			beside: null,
		};
	}
	const leaf = findLeaf(layout.grid.root, layout.grid.orientation, id);
	if (!leaf) return null;
	return {
		group: leaf.group.id,
		index: leaf.group.views.indexOf(id),
		floating: null,
		beside: besideOf(leaf),
	};
};

/**
 * Calls `closed` with where each pane was as you close it. Panes that move
 * or go when a layout loads don't count. Returns a call that stops it.
 */
export const watchClosing = (
	api: DockviewApi,
	closed: (pane: ClosedPane) => void
): (() => void) => {
	let before: SerializedDockview | null = null;
	const listening = [
		api.onWillMutateLayout(({ kind }) => {
			before = null;
			if (kind !== "remove") return;
			try {
				before = api.toJSON();
			} catch {
				before = null;
			}
		}),
		api.onDidMutateLayout(() => {
			before = null;
		}),
		api.onDidRemovePanel((panel) => {
			if (!before) return;
			const place = placeOf(before, panel.id);
			if (place) closed({ params: paneParams(panel.id), ...place });
		}),
	];
	return () => {
		for (const l of listening) l.dispose();
	};
};
