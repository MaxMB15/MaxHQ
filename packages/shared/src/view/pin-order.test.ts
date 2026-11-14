// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the pin order cases,
// with MaxHQ's cases for a new pin and the order of pins.

import { describe, expect, it } from "vitest";

import { agent, NOW } from "../test/build";

import { DEFAULT_INBOX } from "./inbox";
import {
	byPin,
	generateSpreadPinOrderKeys,
	nextPinKey,
	pinOrderKeyBetween,
	planPinnedReorder,
} from "./pin-order";

describe("planPinnedReorder with hidden rows", () => {
	it("keeps hidden slots available when inserting between visible neighbors", () => {
		const midpoint = pinOrderKeyBetween("f", "t")!;
		const keysById = new Map([
			["a", "f"],
			["b", "t"],
			["moved", "z"],
			["snoozed", midpoint],
		]);
		const assignments = planPinnedReorder({
			orderedIds: ["a", "moved", "b"],
			keysById,
			movedId: "moved",
		});
		expect(assignments).toHaveLength(1);
		const key = assignments[0]!.key;
		expect(key > "f" && key < "t").toBe(true);
		expect(key).not.toBe(midpoint);
		expect(assignments[0]!.id).toBe("moved");
	});

	it("rewrites corrupt keys without overwriting hidden slots", () => {
		const reserved = generateSpreadPinOrderKeys(6);
		const keysById = new Map<string, string | null>([
			["a", null],
			["b", null],
			["c", null],
			...reserved.map((key, i) => [`hidden-${i}`, key] as const),
		]);
		const assignments = planPinnedReorder({
			orderedIds: ["c", "a", "b"],
			keysById,
			movedId: "c",
		});
		expect(assignments.map(({ id }) => id)).toEqual(["c", "a", "b"]);
		const keys = assignments.map(({ key }) => key);
		expect(keys).toEqual([...keys].sort());
		expect(new Set(keys).size).toBe(3);
		expect(keys.every((key) => !reserved.includes(key))).toBe(true);
	});

	it("keeps a large list in the order asked for", () => {
		const ids = Array.from({ length: 1_200 }, (_, index) => String(index));
		const orderedIds = [...ids].reverse();
		const assignments = planPinnedReorder({
			orderedIds,
			movedId: orderedIds[0]!,
			keysById: new Map(ids.map((id) => [id, null])),
		});
		const keys = new Map(assignments.map((assignment) => [assignment.id, assignment.key]));
		const sorted = [...ids].sort((a, b) => (keys.get(a)! < keys.get(b)! ? -1 : 1));
		expect(sorted).toEqual(orderedIds);
	});
});

describe("generateSpreadPinOrderKeys", () => {
	it.each([0, 1, 650, 675, 676, 1_001, 2_000])(
		"leaves unique, insertable keys for %i pins",
		(count) => {
			const keys = generateSpreadPinOrderKeys(count);
			expect(keys).toHaveLength(count);
			expect(new Set(keys).size).toBe(count);
			expect([...keys].sort()).toEqual(keys);
			for (let index = 0; index < keys.length; index += 1) {
				const before = keys[index - 1] ?? null;
				const after = keys[index]!;
				expect(after).toMatch(/^[a-z]*[b-z]$/);
				const between = pinOrderKeyBetween(before, after);
				expect(between).not.toBeNull();
				expect(between! < after).toBe(true);
				if (before !== null) expect(between! > before).toBe(true);
			}
		}
	);
});

describe("a new pin", () => {
	it("goes after every pin there is", () => {
		expect(nextPinKey([])).toBe("n");
		const key = nextPinKey(["c", "x", "m"]);
		expect(key > "x").toBe(true);
	});

	it("still goes last after a corrupt key", () => {
		expect(nextPinKey(["ma"]) > "ma").toBe(true);
		expect(nextPinKey(["M"]) > "M").toBe(true);
	});
});

describe("the order of pins", () => {
	const pinned = (id: string, key: string, at: number) =>
		agent({ id, inbox: { ...DEFAULT_INBOX, pin: { at, key } } });

	it("sorts by key as text, then by when each was pinned", () => {
		const pins = [pinned("b", "m", NOW), pinned("a", "m", NOW - 1), pinned("c", "c", NOW)];
		expect(pins.sort(byPin).map((a) => a.id)).toEqual(["c", "a", "b"]);
	});
});
