// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Which account the dispatcher runs as. The pick lives in localStorage,
// since the dispatcher runs on this Mac, and every window takes up the
// others' changes. Without a pick, or once its account is gone, the
// dispatcher takes the first Claude account, else the first Codex one.

import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";

import { type Services, useServices, useView } from "@/app/services";
import type { Account, AccountId } from "@maxhq/shared/bindings";
import { dispatcherAccount } from "@maxhq/shared/dispatch/plan";
import { dispatcherDevice } from "@maxhq/shared/dispatch/reach";

/** The storage key for the account picked in settings. */
export const DISPATCHER_ACCOUNT_KEY = "maxhq.dispatcher.account";

type DispatcherPrefs = { readonly picked: AccountId | null };

const load = (): AccountId | null => {
	try {
		return localStorage.getItem(DISPATCHER_ACCOUNT_KEY);
	} catch {
		return null;
	}
};

const stores = new WeakMap<Services, StoreApi<DispatcherPrefs>>();

const prefsOf = (services: Services): StoreApi<DispatcherPrefs> => {
	let store = stores.get(services);
	if (!store) {
		store = createStore<DispatcherPrefs>(() => ({ picked: load() }));
		stores.set(services, store);
	}
	return store;
};

/** Picks the account the dispatcher runs as and saves it. */
export const setDispatcherAccount = (services: Services, account: AccountId): void => {
	prefsOf(services).setState({ picked: account });
	try {
		localStorage.setItem(DISPATCHER_ACCOUNT_KEY, account);
	} catch (error) {
		console.warn("Couldn't save the dispatcher's account", error);
	}
};

/** The account the dispatcher runs as, or undefined when this Mac has none it can use. */
export const useDispatcherAccount = (): Account | undefined => {
	const picked = useStore(prefsOf(useServices()), (s) => s.picked);
	return useView((s) => dispatcherAccount(s.view, dispatcherDevice(s.view), picked));
};

/** Takes up the account another window picks. Returns a function that stops. */
export const watchDispatcherPrefs = (services: Services): (() => void) => {
	const onStorage = (e: StorageEvent) => {
		if (e.key === DISPATCHER_ACCOUNT_KEY) prefsOf(services).setState({ picked: e.newValue });
	};
	window.addEventListener("storage", onStorage);
	return () => window.removeEventListener("storage", onStorage);
};
