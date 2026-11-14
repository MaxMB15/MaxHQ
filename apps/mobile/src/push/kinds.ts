// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The push token and the kinds you want, kept together so either change
// sends both. The kinds stay in a file in the app's documents; the token
// comes from iOS on each launch.

import { File, Paths } from "expo-file-system";
import { createStore } from "zustand";

import type { PushKind } from "@maxhq/shared/bindings";
import { readKinds } from "@maxhq/shared/push/push";

const kept = () => new File(Paths.document, "push-kinds.json");

const load = (): PushKind[] => {
	try {
		const file = kept();
		return readKinds(file.exists ? file.textSync() : null);
	} catch {
		return readKinds(null);
	}
};

export type PushState = {
	/** The device token iOS gave, once it has. */
	readonly token: string | null;
	readonly kinds: readonly PushKind[];
};

export const pushStore = createStore<PushState>(() => ({ token: null, kinds: load() }));

/** Keeps the kinds you picked and sends them with the token. */
export const keepKinds = (kinds: readonly PushKind[]) => {
	pushStore.setState({ kinds });
	try {
		kept().write(JSON.stringify(kinds));
	} catch {
		// Kept for this run only; the next launch starts with every kind on.
	}
};
