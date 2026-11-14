// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A map whose new versions don't copy it. Each version shares a base map
// with the versions before it and keeps what changed since in a small map
// on top, so changing one entry copies only the few on top. Once the top
// grows past a few dozen entries, the next version folds it into a new base.
// No version ever changes, so it works as React state, and an entry that
// didn't change keeps its identity.

/** Marks an entry the base has and this version removed. */
const GONE: unique symbol = Symbol("gone");

type Slot<V> = V | typeof GONE;

/** How many entries can sit on top before they fold into the base. */
const MOST_ON_TOP = 32;

/** What `key` holds in the version made of `base` and `top`. */
const valueOf = <K, V>(base: ReadonlyMap<K, V>, top: ReadonlyMap<K, Slot<V>>, key: K) => {
	const slot = top.get(key);
	if (slot === undefined) return base.get(key);
	return slot === GONE ? undefined : slot;
};

/** One version. Its values are never undefined, so a lookup needs one read. */
class LayeredMap<K, V extends {}> implements ReadonlyMap<K, V> {
	constructor(
		/** Shared with the versions before. */
		readonly base: ReadonlyMap<K, V>,
		/** What changed since the base. */
		readonly top: ReadonlyMap<K, Slot<V>>,
		readonly size: number
	) {}

	get(key: K): V | undefined {
		return valueOf(this.base, this.top, key);
	}

	has(key: K): boolean {
		return this.get(key) !== undefined;
	}

	/** Each entry, in the base's order, then the ones added since. */
	*entries(): MapIterator<[K, V]> {
		for (const [key, value] of this.base) {
			const slot = this.top.get(key);
			if (slot === undefined) yield [key, value];
			else if (slot !== GONE) yield [key, slot];
		}
		for (const [key, slot] of this.top) {
			if (slot !== GONE && !this.base.has(key)) yield [key, slot];
		}
	}

	*keys(): MapIterator<K> {
		for (const [key] of this.entries()) yield key;
	}

	*values(): MapIterator<V> {
		for (const [, value] of this.entries()) yield value;
	}

	[Symbol.iterator](): MapIterator<[K, V]> {
		return this.entries();
	}

	forEach(callback: (value: V, key: K, map: ReadonlyMap<K, V>) => void, self?: unknown): void {
		for (const [key, value] of this.entries()) callback.call(self, value, key, this);
	}
}

/** The next version of a map, while it's being changed. */
export type Draft<K, V> = {
	readonly get: (key: K) => V | undefined;
	readonly has: (key: K) => boolean;
	readonly set: (key: K, value: V) => void;
	/** Removes an entry, and says whether there was one. */
	readonly delete: (key: K) => boolean;
	/** The new version, or the map it started from when nothing changed. */
	readonly done: () => ReadonlyMap<K, V>;
};

/** Starts the next version of `map`, which can be any map. `map` stays as it is. */
export const draft = <K, V extends {}>(map: ReadonlyMap<K, V>): Draft<K, V> => {
	const from = map instanceof LayeredMap ? (map as LayeredMap<K, V>) : null;
	const base = from ? from.base : map;
	// Copied on the first change, so a draft that changes nothing costs nothing.
	let top: Map<K, Slot<V>> | null = null;
	let size = map.size;
	const get = (key: K) => (top ? valueOf(base, top, key) : map.get(key));
	const edit = () => (top ??= new Map(from?.top));

	return {
		get,
		has: (key) => get(key) !== undefined,
		set: (key, value) => {
			if (get(key) === undefined) size++;
			edit().set(key, value);
		},
		delete: (key) => {
			if (get(key) === undefined) return false;
			size--;
			if (base.has(key)) edit().set(key, GONE);
			else edit().delete(key);
			return true;
		},
		done: () => {
			if (!top) return map;
			const next = new LayeredMap(base, top, size);
			return top.size > MOST_ON_TOP ? new Map(next) : next;
		},
	};
};
