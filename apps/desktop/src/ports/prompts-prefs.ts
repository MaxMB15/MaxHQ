// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Port prompts setting: whether agent views offer to forward or open
// the ports agents start listening on. It lives in localStorage, since it
// belongs to this Mac's view, and every window takes up the others'
// changes.

import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";

import { type Services, useServices } from "@/app/services";
import { t as translate } from "@maxhq/shared/i18n";
import { readPortPrompts, writePortPrompts } from "@maxhq/shared/ports/stack";

/** The storage key. */
export const PORT_PROMPTS_KEY = "maxhq.port-prompts";

type PromptsPrefs = { readonly shown: boolean };

const load = (): string | null => {
	try {
		return localStorage.getItem(PORT_PROMPTS_KEY);
	} catch {
		return null;
	}
};

const stores = new WeakMap<Services, StoreApi<PromptsPrefs>>();

const prefsOf = (services: Services): StoreApi<PromptsPrefs> => {
	let store = stores.get(services);
	if (!store) {
		store = createStore<PromptsPrefs>(() => ({ shown: readPortPrompts(load()) }));
		stores.set(services, store);
	}
	return store;
};

/** Turns the port prompts on or off and saves it. */
export const setPortPrompts = (services: Services, shown: boolean): void => {
	prefsOf(services).setState({ shown });
	try {
		localStorage.setItem(PORT_PROMPTS_KEY, writePortPrompts(shown));
	} catch (error) {
		console.warn(
			translate("Couldn't save {{what}}", { what: translate("the port prompts setting") }),
			error
		);
	}
};

/** Whether agent views show port prompts. */
export const usePortPrompts = (): boolean => useStore(prefsOf(useServices()), (s) => s.shown);

/** Takes up the setting another window saves. Returns a function that stops. */
export const watchPortPrompts = (services: Services): (() => void) => {
	const onStorage = (e: StorageEvent) => {
		if (e.key === PORT_PROMPTS_KEY)
			prefsOf(services).setState({ shown: readPortPrompts(e.newValue) });
	};
	window.addEventListener("storage", onStorage);
	return () => window.removeEventListener("storage", onStorage);
};
