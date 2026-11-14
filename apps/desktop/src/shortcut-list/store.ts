// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Whether the window's list of keyboard shortcuts is open. Cmd+/ opens it
// and closes it again.

import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";

import { type Services, useServices } from "@/app/services";

/** The list's state. */
export type ShortcutListState = { readonly open: boolean };

const stores = new WeakMap<Services, StoreApi<ShortcutListState>>();

/** The list's store for one window. */
export const shortcutListOf = (services: Services): StoreApi<ShortcutListState> => {
	let store = stores.get(services);
	if (!store) {
		store = createStore<ShortcutListState>(() => ({ open: false }));
		stores.set(services, store);
	}
	return store;
};

/** Opens the list, or closes it when it's open. */
export const toggleShortcutList = (services: Services): void =>
	shortcutListOf(services).setState((s) => ({ open: !s.open }));

/** Closes the list. */
export const closeShortcutList = (services: Services): void =>
	shortcutListOf(services).setState({ open: false });

/** Reads the list's state. */
export const useShortcutList = <T>(selector: (state: ShortcutListState) => T): T =>
	useStore(shortcutListOf(useServices()), selector);
