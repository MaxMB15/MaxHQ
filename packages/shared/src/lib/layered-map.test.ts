// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { draft } from "./layered-map";

type Value = { readonly n: number };

/** Everything `map` holds, read every way a map can be read. */
const read = (map: ReadonlyMap<string, Value>, keys: readonly string[]) => {
	const each: [string, Value][] = [];
	map.forEach((value, key) => each.push([key, value]));
	return {
		size: map.size,
		entries: [...map],
		keys: [...map.keys()],
		values: [...map.values()],
		each,
		got: keys.map((key) => [key, map.get(key), map.has(key)]),
	};
};

describe("a layered map", () => {
	it("leaves the version before as it was, and shares what didn't change", () => {
		const a = { n: 1 };
		const b = { n: 2 };
		const first = new Map([
			["a", a],
			["b", b],
		]);
		const change = draft(first);
		change.set("a", { n: 10 });
		change.set("c", { n: 3 });
		expect(change.delete("b")).toBe(true);
		expect(change.delete("b")).toBe(false);
		const second = change.done();

		expect([...first]).toEqual([
			["a", a],
			["b", b],
		]);
		expect([...second]).toEqual([
			["a", { n: 10 }],
			["c", { n: 3 }],
		]);
		expect(second.size).toBe(2);
		expect(second.has("b")).toBe(false);

		const third = draft(second);
		third.set("d", { n: 4 });
		const next = third.done();
		expect(next.get("c")).toBe(second.get("c"));
		expect(second.has("d")).toBe(false);
	});

	it("hands back the same map when nothing changed", () => {
		const map = new Map([["a", { n: 1 }]]);
		const change = draft(map);
		expect(change.delete("missing")).toBe(false);
		expect(change.done()).toBe(map);
	});

	it("reads the same as a plain map through a long run of changes", () => {
		const keys = Array.from({ length: 60 }, (_, n) => `k${n}`);
		let plain = new Map<string, Value>();
		let layered: ReadonlyMap<string, Value> = new Map();
		// The same steps every run, so a failure comes back.
		let seed = 7;
		const random = (below: number) => {
			seed = (seed * 1_103_515_245 + 12_345) % 2_147_483_648;
			return seed % below;
		};
		for (let batch = 0; batch < 300; batch++) {
			const change = draft(layered);
			for (let step = random(6); step >= 0; step--) {
				const key = keys[random(keys.length)] as string;
				if (random(4) === 0) {
					expect(change.delete(key)).toBe(plain.delete(key));
				} else {
					const value = { n: batch * 10 + step };
					change.set(key, value);
					plain.set(key, value);
				}
				expect(change.get(key)).toBe(plain.get(key));
				expect(change.has(key)).toBe(plain.has(key));
			}
			const before = read(layered, keys);
			const next = change.done();
			expect(read(layered, keys)).toEqual(before);
			layered = next;
			expect(new Map(read(layered, keys).entries)).toEqual(plain);
			expect(read(layered, keys).got).toEqual(read(plain, keys).got);
			expect(layered.size).toBe(plain.size);
			plain = new Map(plain);
		}
	});

	it("folds its changes into a new base once they pile up", () => {
		let map: ReadonlyMap<string, Value> = new Map();
		const folded: boolean[] = [];
		for (let n = 0; n < 40; n++) {
			const change = draft(map);
			change.set(`k${n}`, { n });
			map = change.done();
			folded.push(map instanceof Map);
		}
		expect(folded.indexOf(true)).toBe(32);
		expect(map.size).toBe(40);
	});
});
