// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The guide's map, guide/map.json: every Settings page with its words, the
// keyboard shortcuts and the views a link opens, each with the link that
// opens it. maxd's tests write the tools part, and this the rest, so each
// side keeps the other's parts as they are. `pnpm guide:map` rewrites it,
// and `pnpm lint` fails while a part is stale or an entry has no guide
// section that names it.

import type { ViewEntry } from "@maxhq/shared/guide/view-link";

/** A Settings card or view, or a card on a device's tab, in the map. */
export type SettingsEntry = {
	readonly id: string;
	readonly title: string;
	readonly hint?: string;
	/** The labels on it, which the sections that name it must mention. */
	readonly terms: readonly string[];
	readonly link: string;
};

/** A keyboard shortcut in the map. */
export type ShortcutEntry = {
	readonly id: string;
	readonly title: string;
	readonly group: string;
	/** The keys as a Mac's menu writes them. More than one means any of them. */
	readonly keys: readonly string[];
	readonly link: string;
};

/** What the map is made from. */
export type MapSources = {
	readonly settings: readonly SettingsEntry[];
	readonly shortcuts: readonly { what: string; group: string; keys: readonly string[] }[];
	readonly views: readonly ViewEntry[];
};

/** The parts of the map this side writes. */
export type MapParts = {
	readonly settings: readonly SettingsEntry[];
	readonly shortcuts: readonly ShortcutEntry[];
	readonly views: readonly ViewEntry[];
};

/** Where the shortcut entries open: the Settings page that lists them all. */
export const SHORTCUTS_LINK = "maxhq://settings/shortcuts";

/** Lowercase words joined by dashes, for an id. */
export const slug = (text: string): string =>
	text
		.toLowerCase()
		.normalize("NFKD")
		.replace(/\p{M}/gu, "")
		.replace(/[^a-z0-9]+/g, "-")
		.replace(/^-|-$/g, "");

/**
 * The parts of the map. A shortcut's id is its words, and its group's
 * too when two groups have the same words.
 */
export const buildMap = (sources: MapSources): MapParts => {
	const count = new Map<string, number>();
	for (const row of sources.shortcuts)
		count.set(slug(row.what), (count.get(slug(row.what)) ?? 0) + 1);
	const shortcuts = sources.shortcuts.map((row) => {
		const words = slug(row.what);
		const id = count.get(words)! > 1 ? `${slug(row.group)}-${words}` : words;
		return {
			id: `shortcut/${id}`,
			title: row.what,
			group: row.group,
			keys: row.keys,
			link: SHORTCUTS_LINK,
		};
	});
	return { settings: sources.settings, shortcuts, views: sources.views };
};

/** A value with every object's keys sorted, the way maxd writes JSON. */
const sorted = (value: unknown): unknown => {
	if (Array.isArray(value)) return value.map(sorted);
	if (value !== null && typeof value === "object") {
		return Object.fromEntries(
			Object.entries(value)
				.filter(([, inner]) => inner !== undefined)
				.sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
				.map(([key, inner]) => [key, sorted(inner)])
		);
	}
	return value;
};

/** The map's text: `parts` over what `onDisk` has, with sorted keys. */
export const renderMap = (parts: MapParts, onDisk: Record<string, unknown>): string =>
	`${JSON.stringify(sorted({ ...onDisk, ...parts }), null, 2)}\n`;

/** The names of the parts that differ from the map on disk. */
export const staleParts = (parts: MapParts, onDisk: Record<string, unknown>): string[] =>
	Object.entries(parts)
		.filter(
			([name, part]) => JSON.stringify(sorted(part)) !== JSON.stringify(sorted(onDisk[name]))
		)
		.map(([name]) => name);
