// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Hands an agent view's find to its rows, so a row whose item holds the
// active match opens what it folds away while it does. Closing it by hand
// keeps it shut until find moves again.

import { createContext, useContext, useState, type ReactNode } from "react";
import { createStore, type StoreApi, useStore } from "zustand";

import { CLOSED_FIND, type FindState } from "../find/store";
import { useKept } from "./kept";

const FindContext = createContext<StoreApi<FindState> | null>(null);

// What a row reads when no find is around it, as in tests of the row alone.
const NONE = createStore<FindState>(() => CLOSED_FIND);

export const FindProvider = ({
	store,
	children,
}: {
	store: StoreApi<FindState>;
	children: ReactNode;
}) => <FindContext.Provider value={store}>{children}</FindContext.Provider>;

/** Find's move to the item's active match, or null while the item doesn't hold it. */
export const useFindHeld = (item: string): number | null =>
	useStore(useContext(FindContext) ?? NONE, (state) =>
		state.open && state.match?.item === item ? state.turn : null
	);

/**
 * Like `useKept` for whether a row shows something it folds away, but
 * shown while the row's item holds find's active match. The third value
 * says whether it does.
 */
export const useFold = (
	key: string,
	item: string,
	initial = false
): [boolean, (value: boolean) => void, boolean] => {
	const [open, setOpen] = useKept(key, initial);
	const held = useFindHeld(item);
	// The move you closed it on, so it stays shut until find moves again.
	const [shut, setShut] = useState<number | null>(null);
	const shown = open || (held !== null && held !== shut);
	const set = (next: boolean) => {
		setShut(!next && held !== null ? held : null);
		setOpen(next);
	};
	return [shown, set, held !== null];
};
