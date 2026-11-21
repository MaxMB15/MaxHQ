// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a search of Settings finds. The page's cards match by their titles
// and terms, and by their group's name. The views of their own, such as a
// login or a project, match by their titles and terms too. A device
// matches at each of its cards that has the words, or at its tab's top
// when its name alone has them. The hits come back in the order the page
// shows them, with what each group, card, device and phone tab keeps.

import type { DeviceId } from "@maxhq/shared/bindings";
import type {
	DeviceCardId,
	PageCard,
	PhoneCard,
	SettingsDetail,
	SettingsGroup,
} from "@maxhq/shared/guide/settings-link";

import { type Found, findIn } from "../search";

/** Where a hit sits in the left column: a group, or the views of their own. */
export type HitGroup = SettingsGroup | "views";

/** A card of the page as the search sees it. Its text is already translated. */
export type CardEntry = {
	readonly card: PageCard;
	readonly group: SettingsGroup;
	readonly title: string;
	readonly terms: readonly string[];
};

/** A view of its own, or the keyboard shortcuts, as the search sees it. */
export type DetailEntry = {
	/** One hit at most for each key, from the first entry that has the words. */
	readonly key: string;
	readonly group: HitGroup;
	readonly place: SettingsDetail | { readonly kind: "shortcuts" };
	readonly title: string;
	readonly terms: readonly string[];
};

/** A device and the terms of each of its tab's cards. */
export type DeviceEntry = {
	readonly device: DeviceId;
	readonly name: string;
	readonly cards: readonly { readonly id: DeviceCardId; readonly terms: readonly string[] }[];
};

/** A card of the Phones tab. */
export type PhoneEntry = {
	readonly card: PhoneCard;
	readonly title: string;
	readonly terms: readonly string[];
};

/** Everything a search looks through. */
export type SearchIndex = {
	readonly groups: readonly { readonly group: SettingsGroup; readonly label: string }[];
	readonly cards: readonly CardEntry[];
	readonly details: readonly DetailEntry[];
	readonly devices: readonly DeviceEntry[];
	readonly phones: readonly PhoneEntry[];
};

/** One thing the search found. */
export type Hit =
	| {
			readonly kind: "card";
			readonly key: string;
			readonly group: SettingsGroup;
			readonly card: PageCard;
			readonly found: Found;
	  }
	| {
			readonly kind: "detail";
			readonly key: string;
			readonly group: HitGroup;
			readonly place: DetailEntry["place"];
			readonly found: Found;
	  }
	| {
			readonly kind: "device";
			readonly key: string;
			readonly group: "devices";
			readonly device: DeviceId;
			/** The first card with the words, or none when the name matched. */
			readonly card: DeviceCardId | null;
			readonly found: Found;
	  }
	| {
			readonly kind: "phones";
			readonly key: string;
			readonly group: "devices";
			readonly card: PhoneCard;
			readonly found: Found;
	  };

/** What a search keeps of the page. */
export type Results = {
	/** In the order the page shows them. */
	readonly hits: readonly Hit[];
	/** The groups with a hit, which the left column lists. */
	readonly groups: ReadonlySet<HitGroup>;
	/** The page's cards that show. */
	readonly cards: ReadonlySet<PageCard>;
	/** The devices with a hit, each with the cards its tab shows, or null for all of them. */
	readonly devices: ReadonlyMap<DeviceId, ReadonlySet<DeviceCardId> | null>;
	/** The Phones tab's cards that show. */
	readonly phones: ReadonlySet<PhoneCard>;
	/** The projects the list shows, or null for every one. */
	readonly projects: ReadonlySet<string> | null;
};

// A view of its own brings the card that leads to it along: a login the
// accounts table, a project the projects list.
const LEADS_TO: Partial<Record<SettingsDetail["kind"], PageCard>> = {
	login: "accounts",
	project: "projects",
};

/** What `words` find in `index`. With no words, null: the page shows all of itself. */
export const searchSettings = (index: SearchIndex, words: readonly string[]): Results | null => {
	if (words.length === 0) return null;
	const label = new Map(index.groups.map((g) => [g.group, g.label]));
	const order = new Map<HitGroup, number>(index.groups.map((g, at) => [g.group, at]));
	order.set("views", index.groups.length);

	const hits: Hit[] = [];
	const cards = new Set<PageCard>();
	for (const entry of index.cards) {
		const found = findIn(
			{ title: entry.title, terms: [...entry.terms, label.get(entry.group) ?? ""] },
			words
		);
		if (!found) continue;
		cards.add(entry.card);
		hits.push({ kind: "card", key: entry.card, group: entry.group, card: entry.card, found });
	}

	// The projects list shows every project when it matched by itself.
	const everyProject = cards.has("projects");
	const seen = new Set<string>();
	const projects = new Set<string>();
	for (const entry of index.details) {
		if (seen.has(entry.key)) continue;
		const found = findIn({ title: entry.title, terms: entry.terms }, words);
		if (!found) continue;
		seen.add(entry.key);
		hits.push({
			kind: "detail",
			key: entry.key,
			group: entry.group,
			place: entry.place,
			found,
		});
		const leads = entry.place.kind === "shortcuts" ? undefined : LEADS_TO[entry.place.kind];
		if (leads) cards.add(leads);
		if (entry.place.kind === "project") projects.add(entry.place.project);
	}

	// The Devices and phones group's name finds every tab whole.
	const tabs = label.get("devices") ?? "";
	const devices = new Map<DeviceId, ReadonlySet<DeviceCardId> | null>();
	for (const entry of index.devices) {
		const named = findIn({ title: entry.name, terms: [tabs] }, words);
		if (named) {
			devices.set(entry.device, null);
			hits.push(deviceHit(entry, null, named));
			continue;
		}
		const shown = new Set<DeviceCardId>();
		let first: Hit | null = null;
		for (const card of entry.cards) {
			const found = findIn({ title: entry.name, terms: card.terms }, words);
			if (!found) continue;
			shown.add(card.id);
			first ??= deviceHit(entry, card.id, found);
		}
		if (first) {
			devices.set(entry.device, shown);
			hits.push(first);
		}
	}

	const phones = new Set<PhoneCard>();
	for (const entry of index.phones) {
		const found = findIn({ title: entry.title, terms: [...entry.terms, tabs] }, words);
		if (!found) continue;
		phones.add(entry.card);
		hits.push({
			kind: "phones",
			key: `phones-${entry.card}`,
			group: "devices",
			card: entry.card,
			found,
		});
	}

	// In the page's order: by group, and in a group as the page lists them.
	const sorted = hits
		.map((hit, at) => ({ hit, at }))
		.sort((a, b) => order.get(a.hit.group)! - order.get(b.hit.group)! || a.at - b.at)
		.map(({ hit }) => hit);
	return {
		hits: sorted,
		groups: new Set(sorted.map((hit) => hit.group)),
		cards,
		devices,
		phones,
		projects: everyProject ? null : projects,
	};
};

const deviceHit = (entry: DeviceEntry, card: DeviceCardId | null, found: Found): Hit => ({
	kind: "device",
	key: `device-${entry.device}`,
	group: "devices",
	device: entry.device,
	card,
	found,
});

/**
 * The tab the Devices and phones group shows during a search: the one you
 * picked when it has a hit, else the first device with one, else Phones.
 * Null when neither a device nor a phone card matched.
 */
export const searchTab = (
	tab: DeviceId | "phones",
	results: Results
): DeviceId | "phones" | null => {
	if (tab === "phones" ? results.phones.size > 0 : results.devices.has(tab)) return tab;
	const first = results.devices.keys().next();
	if (!first.done) return first.value;
	return results.phones.size > 0 ? "phones" : null;
};
