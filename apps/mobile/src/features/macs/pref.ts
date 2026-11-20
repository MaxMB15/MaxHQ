// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How this phone shows several Macs, and the Mac it shows one at a time.
// It stays in a file in the app's documents, as the dispatcher's settings
// do.

import { File, Paths } from "expo-file-system";
import { createStore } from "zustand";

import { DEFAULT_MACS_PREF, type MacsPref, macsPrefOf } from "./macs";

const kept = () => new File(Paths.document, "macs.json");

const load = (): MacsPref => {
	try {
		const file = kept();
		return macsPrefOf(file.exists ? file.textSync() : null);
	} catch {
		return DEFAULT_MACS_PREF;
	}
};

/** This phone's choice. */
export const macsPref = createStore<MacsPref>(load);

/** Keeps a change to the choice. */
export const keepMacsPref = (change: Partial<MacsPref>) => {
	macsPref.setState(change);
	try {
		kept().write(JSON.stringify(macsPref.getState()));
	} catch {
		// Kept for this run only; the next launch shows the Macs together.
	}
};
