// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Open streams shared by every view of the same key, such as an agent's
// timeline or a workflow run. One stays open for a minute after its last
// view goes, so a pane hidden and shown again finds it where it left it.

import type { DataSource, Unsubscribe } from "./source";

export const LINGER_MS = 60_000;

type Entry<T> = {
	readonly value: T;
	users: number;
	timer: ReturnType<typeof setTimeout> | null;
};

/** A value one view uses, and the call that says the view is done with it. */
export type Held<T> = { readonly value: T; readonly release: () => void };

/** The opens of one kind, shared by key. */
export type SharedOpens<K, T> = {
	/** The open value for a key, opened if it isn't. Safe to call while rendering. */
	readonly shared: (source: DataSource, key: K, linger?: number) => T;
	/** Holds the open value for a key until released. */
	readonly hold: (source: DataSource, key: K, linger?: number) => Held<T>;
};

/** Shares what `open` opens, by `name(key)`, closing each once nobody holds it. */
export const sharedOpens = <K, T extends { readonly close: Unsubscribe }>(
	open: (source: DataSource, key: K) => T,
	name: (key: K) => string
): SharedOpens<K, T> => {
	const bySource = new WeakMap<DataSource, Map<string, Entry<T>>>();

	// Closes the entry once it has had no users for `linger` milliseconds.
	const closeLater = (
		entries: Map<string, Entry<T>>,
		id: string,
		entry: Entry<T>,
		linger: number
	) => {
		if (entry.timer) clearTimeout(entry.timer);
		entry.timer = setTimeout(() => {
			if (entry.users > 0) return;
			entry.value.close();
			if (entries.get(id) === entry) entries.delete(id);
		}, linger);
	};

	const entryFor = (source: DataSource, key: K, linger: number) => {
		let entries = bySource.get(source);
		if (!entries) {
			entries = new Map();
			bySource.set(source, entries);
		}
		const id = name(key);
		let entry = entries.get(id);
		if (!entry) {
			entry = { value: open(source, key), users: 0, timer: null };
			entries.set(id, entry);
			// Nobody holds it yet. If nobody comes, it closes like any other.
			closeLater(entries, id, entry, linger);
		}
		return { entries, id, entry };
	};

	return {
		shared: (source, key, linger = LINGER_MS) => entryFor(source, key, linger).entry.value,
		hold: (source, key, linger = LINGER_MS) => {
			const { entries, id, entry } = entryFor(source, key, linger);
			entry.users++;
			if (entry.timer) clearTimeout(entry.timer);
			entry.timer = null;
			let released = false;
			return {
				value: entry.value,
				release: () => {
					if (released) return;
					released = true;
					entry.users--;
					if (entry.users === 0) closeLater(entries, id, entry, linger);
				},
			};
		},
	};
};
