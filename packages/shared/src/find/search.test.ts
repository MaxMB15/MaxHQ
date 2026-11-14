// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { createMockTimelines, STREAMING } from "../data/mock-timeline";
import {
	findLines,
	findOccurrences,
	findQueryOf,
	nearItemAt,
	NEAR_ITEMS,
	selectFind,
	stepFindIndex,
} from "./search";

describe("findOccurrences", () => {
	it("finds matches that don't overlap, ignoring case", () => {
		expect(findOccurrences("Deploy the Deployment", "deploy")).toEqual([0, 11]);
		expect(findOccurrences("aaaa", "aa")).toEqual([0, 2]);
		expect(findOccurrences("", "a")).toEqual([]);
		expect(findOccurrences("a", "")).toEqual([]);
	});

	it("keeps offsets when a lowercase letter is longer", () => {
		expect(findOccurrences("İİİ needle", "needle")).toEqual([4]);
		expect(findOccurrences("İ ΟΣ", "Σ")).toEqual([3]);
	});

	it("matches both sigma forms", () => {
		expect(findOccurrences("ΟΣ ος οσ", "Σ")).toEqual([1, 4, 7]);
		expect(findOccurrences("ΟΣ ος οσ", "ς")).toEqual([1, 4, 7]);
	});

	it("drops only the query's outer whitespace", () => {
		expect(findQueryOf("  two  words ")).toBe("two  words");
	});
});

describe("findLines", () => {
	it("breaks at line breaks and reads each run of whitespace as one space", () => {
		expect(findLines("  one \t two\n\n three  ").map((line) => line.text)).toEqual([
			"one two",
			"three",
		]);
	});

	it("says where each character came from", () => {
		const [first, second] = findLines(" a  b\nc");
		expect(first).toEqual({ text: "a b", at: [1, 2, 4] });
		expect(second).toEqual({ text: "c", at: [6] });
	});

	it("reads only whitespace as no lines", () => {
		expect(findLines("")).toEqual([]);
		expect(findLines(" \n ")).toEqual([]);
	});
});

describe("stepFindIndex", () => {
	it("wraps round both ways", () => {
		expect(stepFindIndex(0, 3, -1)).toBe(2);
		expect(stepFindIndex(2, 3, 1)).toBe(0);
		expect(stepFindIndex(1, 3, 5)).toBe(0);
		expect(stepFindIndex(4, 0, 1)).toBe(0);
	});
});

const counts = (...each: number[]) => each.map((count, i) => ({ item: `item-${i}`, count }));

describe("selectFind", () => {
	it("holds an index to the last match", () => {
		const found = selectFind(counts(2, 0, 3), { type: "index", index: 99 });
		expect(found).toMatchObject({
			total: 5,
			active: 4,
			found: { item: "item-2", occurrence: 2 },
		});
		expect(found.near).toEqual([
			{ item: "item-0", first: 0, count: 2 },
			{ item: "item-2", first: 2, count: 3 },
		]);
	});

	it("starts from the first match at or after a match, then steps", () => {
		const at = (item: string, occurrence: number, offset: number) =>
			selectFind(counts(2, 0, 3), { type: "from", from: { item, occurrence }, offset })
				.active;
		expect(at("item-1", 0, 0)).toBe(2);
		expect(at("item-0", 1, 1)).toBe(2);
		expect(at("item-2", 3, 0)).toBe(0);
		expect(at("item-0", 0, -1)).toBe(4);
		// An item the timeline doesn't have counts from the first match.
		expect(at("item-9", 0, 1)).toBe(1);
	});

	it("names no match when there are none", () => {
		expect(selectFind(counts(0, 0), { type: "index", index: 0 })).toEqual({
			total: 0,
			active: 0,
			found: null,
			near: [],
		});
	});

	it("puts the active item ninth among the items near it, wrapping round", () => {
		const found = selectFind(counts(...Array.from({ length: 30 }, () => 1)), {
			type: "index",
			index: 2,
		});
		expect(found.near).toHaveLength(NEAR_ITEMS);
		expect(found.near[8]?.item).toBe("item-2");
		expect(found.near[0]?.item).toBe("item-24");
		expect(nearItemAt(found.near, 10)?.item).toBe("item-10");
		expect(nearItemAt(found.near, 11)).toBeUndefined();
	});
});

describe("the sample timelines' find", () => {
	it("counts across the whole timeline", () => {
		const timelines = createMockTimelines({ now: () => 0, stream: false });
		const found = timelines.find(STREAMING, " the ", { type: "index", index: 0 });
		expect(found.total).toBeGreaterThan(0);
		expect(found.active).toBe(0);
		expect(found.found?.occurrence).toBe(0);
	});
});
