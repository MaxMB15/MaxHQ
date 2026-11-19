// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A read from a device for a screen, as the desktop's queries do it: what
// came, why it didn't, whether one is on its way, and a way to read again
// for pull to refresh. What came stays on screen while it's read again,
// and only a new key starts blank. A list that comes a page at a time
// reads the next page on asking. A merge or an approval drops what was
// read of every PR, so the screens showing one read again.

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

/** What a read has come to. */
export type Reading<T> = {
	/** What came, or undefined before the first read of this key comes. */
	readonly data: T | undefined;
	/** Why the latest read failed, in words, or null. */
	readonly error: string | null;
	/** Whether a read is on its way. */
	readonly loading: boolean;
	/** Reads again, settling once that read has. */
	readonly reload: () => Promise<void>;
};

type Held<T> = {
	readonly key: string | null;
	readonly data: T | undefined;
	readonly error: string | null;
	readonly loading: boolean;
};

const messageOf = (error: unknown): string =>
	error instanceof Error ? error.message : String(error);

/**
 * Runs `read` for `key`, and again when `again` changes. A null key reads
 * nothing. A read that finishes after a newer one started is dropped.
 */
export const useRead = <T>(key: string | null, read: () => Promise<T>, again = 0): Reading<T> => {
	const latest = useRef(read);
	// The newest `read`, for the effects and handlers that run after this render.
	useLayoutEffect(() => {
		latest.current = read;
	});
	const asked = useRef(0);
	const [held, setHeld] = useState<Held<T>>({
		key: null,
		data: undefined,
		error: null,
		loading: false,
	});

	const run = useCallback(async (k: string) => {
		const mine = ++asked.current;
		setHeld((s) => ({
			key: k,
			data: s.key === k ? s.data : undefined,
			error: s.key === k ? s.error : null,
			loading: true,
		}));
		try {
			const data = await latest.current();
			if (mine === asked.current) setHeld({ key: k, data, error: null, loading: false });
		} catch (error) {
			if (mine !== asked.current) return;
			setHeld((s) => ({
				key: k,
				data: s.key === k ? s.data : undefined,
				error: messageOf(error),
				loading: false,
			}));
		}
	}, []);

	useEffect(() => {
		if (key !== null) void run(key);
	}, [key, again, run]);
	// A read that comes after the screen went changes nothing.
	useEffect(
		() => () => {
			asked.current += 1;
		},
		[]
	);

	const reload = useCallback(() => (key === null ? Promise.resolve() : run(key)), [key, run]);
	const current = held.key === key;
	return {
		data: current ? held.data : undefined,
		error: current ? held.error : null,
		loading: key !== null && (!current || held.loading),
		reload,
	};
};

/** One page of a list, and whether another follows it. */
export type Page<T> = { readonly items: readonly T[]; readonly more: boolean };

/** What a read a page at a time has come to. */
export type Paging<T> = Omit<Reading<readonly T[]>, "loading"> & {
	/** Whether another page follows the ones read. */
	readonly more: boolean;
	/** Whether the next page is on its way. */
	readonly reading: boolean;
	/** Why the next page didn't come, in words, or null. */
	readonly nextError: string | null;
	/** Reads the next page. */
	readonly next: () => void;
};

type Rest<T> = {
	/** The first page the rest follow. */
	readonly after: Page<T> | undefined;
	readonly pages: readonly Page<T>[];
	readonly error: string | null;
	readonly reading: boolean;
};

/**
 * Runs `read` for the first page of `key`, as `useRead` does, and for each
 * next page on asking. Pages start from 1. Reading the first page again
 * drops the pages after it.
 */
export const usePaged = <T>(
	key: string | null,
	read: (page: number) => Promise<Page<T>>,
	again = 0
): Paging<T> => {
	const latest = useRef(read);
	// The newest `read`, for the effects and handlers that run after this render.
	useLayoutEffect(() => {
		latest.current = read;
	});
	const first = useRead(key, () => latest.current(1), again);
	const [rest, setRest] = useState<Rest<T>>({
		after: undefined,
		pages: [],
		error: null,
		reading: false,
	});
	const mine = first.data !== undefined && rest.after === first.data;
	const pages = mine ? rest.pages : [];
	const more = (pages[pages.length - 1] ?? first.data)?.more ?? false;
	const reading = mine && rest.reading;

	const next = () => {
		const start = first.data;
		if (!start || !more || reading) return;
		setRest({ after: start, pages, error: null, reading: true });
		latest.current(pages.length + 2).then(
			(page) =>
				setRest((s) =>
					s.after === start
						? { after: start, pages: [...s.pages, page], error: null, reading: false }
						: s
				),
			(error: unknown) =>
				setRest((s) =>
					s.after === start ? { ...s, error: messageOf(error), reading: false } : s
				)
		);
	};

	return {
		data: first.data && [first.data, ...pages].flatMap((page) => page.items),
		error: first.error,
		reload: first.reload,
		more,
		reading,
		nextError: mine ? rest.error : null,
		next,
	};
};

const drops = createStore<{ readonly count: number }>(() => ({ count: 0 }));

/** Drops what was read of every PR, so the screens showing one read it again. */
export const dropPulls = (): void => drops.setState((s) => ({ count: s.count + 1 }));

/** Counts the drops, for `useRead`'s `again`. */
export const usePullDrops = (): number => useStore(drops, (s) => s.count);
