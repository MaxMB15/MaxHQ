// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A shared open held while the calling view is mounted.

import { useCallback, useSyncExternalStore } from "react";

import type { SharedOpens } from "./shared-opens";
import type { DataSource } from "./source";

/**
 * The key's open value, held for as long as the calling view is mounted.
 * Pass a key that keeps its identity while it names the same thing, such
 * as a string or a key from the view.
 */
export const useSharedOpen = <K, T>(opens: SharedOpens<K, T>, source: DataSource, key: K): T => {
	const hold = useCallback(
		(changed: () => void) => {
			const held = opens.hold(source, key);
			// One that closed since the render opens again here, so look again.
			changed();
			return held.release;
		},
		[opens, source, key]
	);
	const current = useCallback(() => opens.shared(source, key), [opens, source, key]);
	return useSyncExternalStore(hold, current);
};
