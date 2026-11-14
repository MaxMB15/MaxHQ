// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: matching pull requests
// for the composer, taken to issues too. The `#` menu shows the exact number
// first, then the ones linked to the agent, then the rest in the order the
// lookup gave them, which is the most recently updated first. Words match
// the title, so a stale answer narrows while a new one is on its way.

import type { GitItemRef } from "./draft";

const keyOf = (item: GitItemRef) => `${item.repo.trim().toLowerCase()}#${item.number}`;

/** Whether an item matches what follows the `#`. */
export const matchesGitQuery = (item: GitItemRef, query: string): boolean => {
	if (/^\d+$/.test(query)) return String(item.number).includes(query);
	const words = query
		.toLowerCase()
		.split(/[\s_-]+/u)
		.filter(Boolean);
	const text = `#${item.number} ${item.title}`.toLowerCase();
	return words.every((word) => text.includes(word));
};

/** The `#` menu's items for a query, best first. */
export const rankGitItems = (input: {
	readonly items: readonly GitItemRef[];
	readonly linked?: readonly GitItemRef[];
	readonly query: string;
	readonly limit: number;
}): GitItemRef[] => {
	const linked = (input.linked ?? []).filter((item) => matchesGitQuery(item, input.query));
	const linkedKeys = new Set(linked.map(keyOf));
	const unique = new Map<string, GitItemRef>();
	for (const item of [
		...input.items.filter((item) => matchesGitQuery(item, input.query)),
		...linked,
	])
		if (!unique.has(keyOf(item))) unique.set(keyOf(item), item);
	const isExact = (item: GitItemRef) => String(item.number) === input.query;
	const isLinked = (item: GitItemRef) => linkedKeys.has(keyOf(item));
	// The sort is stable, so items that tie keep the lookup's order.
	return [...unique.values()]
		.sort(
			(left, right) =>
				Number(isExact(right)) - Number(isExact(left)) ||
				Number(isLinked(right)) - Number(isLinked(left))
		)
		.slice(0, input.limit);
};
