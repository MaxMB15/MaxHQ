// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A visual's place among its versions. A visual that `replaces` another
// is its next version, so the first one is v1 and each new one counts up
// along the chain. A replaced visual folds to one line under the timeline's
// newer one. The index is made once per items map, since the map is new
// whenever an item changes.

import { t } from "../i18n";

import type { ItemId, TimelineItem, TurnId } from "../bindings";

export type VisualVersions = {
	/** The visual's version, from 1. */
	readonly version: (id: ItemId) => number;
	/** The visual that replaced this one, or null when it's the newest. */
	readonly replacedBy: (id: ItemId) => ItemId | null;
	/** Whether a turn showed a visual. */
	readonly turnHasVisual: (turn: TurnId) => boolean;
};

type Items = ReadonlyMap<ItemId, TimelineItem>;

const indexes = new WeakMap<Items, VisualVersions>();

// Past this many steps the chain is taken to loop.
const MOST_VERSIONS = 1000;

const build = (items: Items): VisualVersions => {
	const replaces = new Map<ItemId, ItemId>();
	const replacedBy = new Map<ItemId, ItemId>();
	const turns = new Set<TurnId>();
	for (const item of items.values()) {
		if (item.kind !== "agent_visual") continue;
		if (item.turn !== null) turns.add(item.turn);
		const earlier = item.replaces ?? null;
		if (earlier === null || items.get(earlier)?.kind !== "agent_visual") continue;
		replaces.set(item.id, earlier);
		// The newest replacement wins when two name the same one.
		const other = replacedBy.get(earlier);
		if (other === undefined || (items.get(other)?.ordinal ?? -1) < item.ordinal)
			replacedBy.set(earlier, item.id);
	}
	const versions = new Map<ItemId, number>();
	const version = (id: ItemId): number => {
		const known = versions.get(id);
		if (known !== undefined) return known;
		let count = 1;
		for (
			let at = replaces.get(id);
			at !== undefined && count < MOST_VERSIONS;
			at = replaces.get(at)
		)
			count++;
		versions.set(id, count);
		return count;
	};
	return {
		version,
		replacedBy: (id) => replacedBy.get(id) ?? null,
		turnHasVisual: (turn) => turns.has(turn),
	};
};

/** The versions of every visual in a timeline's items. */
export const visualVersions = (items: Items): VisualVersions => {
	let index = indexes.get(items);
	if (!index) {
		index = build(items);
		indexes.set(items, index);
	}
	return index;
};

/** The version slot a card shows, such as "v2", or nothing for a first version. */
export const versionLabel = (version: number): string | null =>
	version > 1 ? `v${version}` : null;

/** The line a replaced visual folds to, such as "Weekly use, v2 · replaced by the one below". */
export const replacedLine = (title: string, version: number): string =>
	t("{{title}}, {{version}} · replaced by the one below", {
		title: title || t("Visual"),
		version: version > 1 ? `v${version}` : t("first version"),
	});
