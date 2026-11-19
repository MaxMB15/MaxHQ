// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The agent list's settings on this phone: the Working shelf and the
// snooze menu's choices. They stay in a file in the app's documents, as
// the Macs choice does.

import { File, Paths } from "expo-file-system";
import { createStore, useStore } from "zustand";

import { DEFAULT_INBOX_PREFS, type InboxPrefs, inboxPrefsOf } from "./inbox-prefs";

const kept = () => new File(Paths.document, "inbox.json");

const load = (): InboxPrefs => {
	try {
		const file = kept();
		return inboxPrefsOf(file.exists ? file.textSync() : null);
	} catch {
		return DEFAULT_INBOX_PREFS;
	}
};

/** This phone's agent list settings. */
export const inboxPrefs = createStore<InboxPrefs>(load);

/** Reads part of the settings, following changes. */
export const useInboxPrefs = <T>(pick: (prefs: InboxPrefs) => T): T => useStore(inboxPrefs, pick);

/** Keeps a change to the settings. */
export const keepInboxPrefs = (change: Partial<InboxPrefs>) => {
	inboxPrefs.setState(change);
	try {
		kept().write(JSON.stringify(inboxPrefs.getState()));
	} catch {
		// Kept for this run only; the next launch starts from the defaults.
	}
};
