// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The dispatcher's settings on this phone: whether its bar always shows,
// and the Mac account it runs as when you picked one. They stay in a file
// in the app's documents, as the push kinds do.

import { File, Paths } from "expo-file-system";
import { createStore } from "zustand";

import type { AccountId } from "@maxhq/shared/bindings";

import { type BarSetting, DEFAULT_PREFS, type DispatcherPrefs, readPrefs } from "./bar";

const kept = () => new File(Paths.document, "dispatcher.json");

const load = (): DispatcherPrefs => {
	try {
		const file = kept();
		return readPrefs(file.exists ? file.textSync() : null);
	} catch {
		return DEFAULT_PREFS;
	}
};

export const dispatcherPrefs = createStore<DispatcherPrefs>(load);

const keep = (change: Partial<DispatcherPrefs>) => {
	dispatcherPrefs.setState(change);
	try {
		kept().write(JSON.stringify(dispatcherPrefs.getState()));
	} catch {
		// Kept for this run only; the next launch starts from the defaults.
	}
};

/** Keeps whether the bar always shows. */
export const keepBar = (bar: BarSetting) => keep({ bar });

/** Keeps the account the dispatcher runs as. */
export const keepAccount = (account: AccountId) => keep({ account });
