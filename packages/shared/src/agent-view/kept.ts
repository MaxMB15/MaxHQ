// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// State a row keeps after it scrolls out of view and unmounts, such as
// whether its output is open or the reason typed into a card. It lives in
// one map for the window, with the oldest entries dropped past a limit.

import { useCallback, useState } from "react";

const LIMIT = 2000;

const kept = new Map<string, unknown>();

/** Keeps a value under its key. */
export const keep = (key: string, value: unknown) => {
	kept.delete(key);
	kept.set(key, value);
	if (kept.size > LIMIT) kept.delete(kept.keys().next().value as string);
};

/** The value kept under a key, or `initial` when there's none. */
export const keptValue = <T>(key: string, initial: T): T =>
	kept.has(key) ? (kept.get(key) as T) : initial;

/** Like `useState`, but the value outlives the component, by key. */
export const useKept = <T>(key: string, initial: T): [T, (value: T) => void] => {
	const [value, setValue] = useState<T>(() => keptValue(key, initial));
	const set = useCallback(
		(next: T) => {
			keep(key, next);
			setValue(next);
		},
		[key]
	);
	return [value, set];
};

/** Forgets everything kept, for tests. */
export const clearKept = () => kept.clear();
