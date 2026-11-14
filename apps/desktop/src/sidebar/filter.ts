// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What you typed in the sidebar's filter field. Each window has its own,
// it lasts while the window does, and it's never saved. Hiding the
// sidebar or switching to devices keeps it.
//
// While it has words, every project and shelf shows its matches, folded or
// not, as a tree's filter does. Folding one then lasts until the filter
// clears, and each is back as you left it.

import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";

import { type Services, useServices } from "@/app/services";
import { searchWords } from "@maxhq/shared/lib/words";
import type { RowsFilter } from "@maxhq/shared/ui/rows";

import { prefsOf, setPrefs } from "./prefs";

/** The filter field's id, for the key that focuses it. */
export const FILTER_ID = "sidebar-filter";

type Filter = {
	readonly query: string;
	/** The projects and shelves folded while the filter has words. */
	readonly folded: ReadonlySet<string>;
};

const stores = new WeakMap<Services, StoreApi<Filter>>();

const filterOf = (services: Services): StoreApi<Filter> => {
	let store = stores.get(services);
	if (!store) {
		store = createStore<Filter>(() => ({ query: "", folded: new Set() }));
		stores.set(services, store);
	}
	return store;
};

/** What the filter field holds. */
export const useFilter = (): string => useStore(filterOf(useServices()), (f) => f.query);

/** The filter's words and folds now, for the rows a Shift-click picks along. */
export const rowsFilter = (services: Services): RowsFilter => {
	const { query, folded } = filterOf(services).getState();
	return { words: searchWords(query), folded };
};

/** Puts text in the filter field. */
export const setFilter = (services: Services, query: string): void =>
	filterOf(services).setState((f) => ({
		query,
		folded: searchWords(query).length > 0 ? f.folded : new Set(),
	}));

/**
 * Whether a project or shelf shows its rows, and what its header does:
 * `open` and `toggle` as you left them, or, while the filter has words,
 * open until you fold it then.
 */
export const useFilterFold = (
	key: string,
	open: boolean,
	toggle: () => void
): readonly [boolean, () => void] => {
	const store = filterOf(useServices());
	const filtering = useStore(store, (f) => searchWords(f.query).length > 0);
	const folded = useStore(store, (f) => f.folded.has(key));
	if (!filtering) return [open, toggle];
	const fold = () =>
		store.setState((f) => {
			const next = new Set(f.folded);
			if (!next.delete(key)) next.add(key);
			return { folded: next };
		});
	return [!folded, fold];
};

/**
 * Brings you to the filter field: shows the sidebar and its projects, then
 * focuses the field with its text selected once it has drawn.
 */
export const focusFilter = (services: Services): void => {
	const ui = services.ui.getState();
	if (ui.mode !== "projects") ui.setMode("projects");
	if (!prefsOf(services).getState().open) setPrefs(services, { open: true });
	setTimeout(() => {
		const field = document.getElementById(FILTER_ID);
		if (!(field instanceof HTMLInputElement)) return;
		field.focus();
		field.select();
	}, 0);
};
