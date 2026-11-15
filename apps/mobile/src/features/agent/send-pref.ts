// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the send button does on this phone while an agent works, Queue or
// Steer. It stays in a file in the app's documents, as the dispatcher's
// settings do.

import { File, Paths } from "expo-file-system";
import { createStore } from "zustand";

import { type BusySend, busySendOf } from "@maxhq/shared/composer/send";

const kept = () => new File(Paths.document, "busy-send.txt");

const load = (): BusySend => {
	try {
		const file = kept();
		return busySendOf(file.exists ? file.textSync().trim() : null);
	} catch {
		return busySendOf(null);
	}
};

/** This phone's pick. */
export const busySendPref = createStore<{ readonly pick: BusySend }>(() => ({ pick: load() }));

/** Keeps the pick. */
export const keepBusySend = (pick: BusySend) => {
	busySendPref.setState({ pick });
	try {
		kept().write(pick);
	} catch {
		// Kept for this run only; the next launch queues again.
	}
};
