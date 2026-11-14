// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { FindAt, TimelineFind } from "../bindings";
import { selectFind } from "./search";
import {
	CLOSED_FIND,
	createFind,
	findLabel,
	REFRESH_MS,
	TYPE_MS,
	type Find,
	type FindAsk,
	type FindState,
} from "./store";

// Thirty items with one match each, so an answer names only some of them.
const counts = Array.from({ length: 30 }, (_, i) => ({ item: `m${i}`, count: 1 }));

type Ask = { query: string; at: FindAt; answer: (found?: TimelineFind) => void; fail: () => void };

/** An ask that waits until the test answers it, from `counts` by default. */
const asker = () => {
	const asks: Ask[] = [];
	const ask: FindAsk = (query, at) =>
		new Promise((resolve, reject) => {
			asks.push({
				query,
				at,
				answer: (found) => resolve(found ?? selectFind(counts, at)),
				fail: () => reject(new Error("maxd went away")),
			});
		});
	return { asks, ask };
};

const settle = () => vi.advanceTimersByTimeAsync(0);

describe("createFind", () => {
	let find: Find;
	let asks: Ask[];

	beforeEach(() => {
		vi.useFakeTimers();
		const made = asker();
		asks = made.asks;
		find = createFind(made.ask);
	});

	afterEach(() => {
		find.dispose();
		vi.useRealTimers();
	});

	const state = () => find.store.getState();

	/** Opens the bar, types `typed` and answers the first ask. */
	const search = async (typed = "match") => {
		find.open();
		find.type(typed);
		await vi.advanceTimersByTimeAsync(TYPE_MS);
		asks.at(-1)!.answer();
		await settle();
	};

	it("waits for typing to pause, then asks from the top of the view", async () => {
		find.readFrom(() => ({ item: "m4", occurrence: 0 }));
		find.open();
		find.type("ma");
		await vi.advanceTimersByTimeAsync(TYPE_MS - 10);
		find.type("  match ");
		await vi.advanceTimersByTimeAsync(TYPE_MS - 10);
		expect(asks).toHaveLength(0);
		await vi.advanceTimersByTimeAsync(10);
		expect(asks).toHaveLength(1);
		expect(asks[0]).toMatchObject({
			query: "match",
			at: { type: "from", from: { item: "m4", occurrence: 0 }, offset: 0 },
		});
		asks[0]!.answer();
		await settle();
		expect(state()).toMatchObject({
			query: "match",
			index: 4,
			match: { item: "m4", occurrence: 0 },
			status: "idle",
		});
	});

	it("starts from the first match before a list says where the view is", async () => {
		await search();
		expect(asks[0]!.at).toEqual({ type: "index", index: 0 });
		expect(state().match).toEqual({ item: "m0", occurrence: 0 });
	});

	it("steps among the items the answer named without asking", async () => {
		await search();
		const turn = state().turn;
		find.step(1);
		find.step(1);
		expect(state()).toMatchObject({ index: 2, match: { item: "m2", occurrence: 0 } });
		expect(state().turn).toBe(turn + 2);
		find.step(-3);
		expect(state()).toMatchObject({ index: 29, match: { item: "m29", occurrence: 0 } });
		expect(asks).toHaveLength(1);
	});

	it("wraps from the last match to the first", async () => {
		await search();
		find.step(-1);
		expect(state()).toMatchObject({ index: 29, match: { item: "m29", occurrence: 0 } });
		find.step(1);
		expect(state()).toMatchObject({ index: 0, match: { item: "m0", occurrence: 0 } });
		expect(asks).toHaveLength(1);
	});

	it("starts again from the view for a new query", async () => {
		await search();
		find.step(3);
		expect(state().index).toBe(3);
		find.readFrom(() => undefined);
		find.type("other");
		expect(state()).toMatchObject({
			query: "",
			result: null,
			index: 0,
			match: null,
			status: "searching",
		});
		await vi.advanceTimersByTimeAsync(TYPE_MS);
		expect(asks[1]).toMatchObject({ query: "other", at: { type: "index", index: 0 } });
		asks[1]!.answer();
		await settle();
		expect(state()).toMatchObject({ query: "other", index: 0, match: { item: "m0" } });
	});

	it("asks from the match once a step goes past the items named", async () => {
		await search();
		// The answer names m22 to m29 and m0 to m8.
		for (let i = 0; i < 8; i++) find.step(1);
		expect(asks).toHaveLength(1);
		find.step(1);
		expect(asks).toHaveLength(2);
		expect(asks[1]!.at).toEqual({
			type: "from",
			from: { item: "m8", occurrence: 0 },
			offset: 1,
		});
		// A step while that ask is out adds to it.
		find.step(1);
		expect(asks[2]!.at).toEqual({
			type: "from",
			from: { item: "m8", occurrence: 0 },
			offset: 2,
		});
		asks[2]!.answer();
		await settle();
		expect(state()).toMatchObject({ index: 10, match: { item: "m10", occurrence: 0 } });
	});

	it("asks again on a step once the timeline changed", async () => {
		await search();
		find.changed();
		find.step(1);
		expect(asks).toHaveLength(2);
		expect(asks[1]!.at).toEqual({
			type: "from",
			from: { item: "m0", occurrence: 0 },
			offset: 1,
		});
	});

	it("asks again a moment after the timeline changes, at most once", async () => {
		await search();
		find.changed();
		await vi.advanceTimersByTimeAsync(REFRESH_MS / 2);
		find.changed();
		await vi.advanceTimersByTimeAsync(REFRESH_MS / 2);
		expect(asks).toHaveLength(2);
		expect(asks[1]!.at).toEqual({
			type: "from",
			from: { item: "m0", occurrence: 0 },
			offset: 0,
		});
		const turn = state().turn;
		asks[1]!.answer();
		await settle();
		// The match stayed where it was, so the view doesn't move.
		expect(state().turn).toBe(turn);
	});

	it("doesn't ask while the bar is closed", async () => {
		find.changed();
		await vi.advanceTimersByTimeAsync(REFRESH_MS);
		expect(asks).toHaveLength(0);
	});

	it("drops an answer to an older ask", async () => {
		find.open();
		find.type("old");
		await vi.advanceTimersByTimeAsync(TYPE_MS);
		find.type("new");
		await vi.advanceTimersByTimeAsync(TYPE_MS);
		expect(asks.map((ask) => ask.query)).toEqual(["old", "new"]);
		asks[1]!.answer();
		await settle();
		asks[0]!.answer({ total: 1, active: 0, found: { item: "m9", occurrence: 0 }, near: [] });
		await settle();
		expect(state()).toMatchObject({ query: "new", match: { item: "m0", occurrence: 0 } });
	});

	it("says a failed ask failed, and asks the same again on retry", async () => {
		await search();
		find.step(-9);
		asks[1]!.fail();
		await settle();
		expect(state().status).toBe("failed");
		find.retry();
		expect(asks[2]!.at).toEqual(asks[1]!.at);
	});

	it("forgets the query on close, and drops what's still to come", async () => {
		find.open();
		find.type("match");
		await vi.advanceTimersByTimeAsync(TYPE_MS);
		find.close();
		asks[0]!.answer();
		await settle();
		expect(state()).toMatchObject({ open: false, typed: "", query: "", result: null });
	});

	it("focuses the input again, and starts from the view, when it's open", async () => {
		await search();
		const focus = state().focus;
		find.readFrom(() => ({ item: "m5", occurrence: 0 }));
		find.open();
		expect(state().focus).toBe(focus + 1);
		expect(asks[1]!.at).toEqual({
			type: "from",
			from: { item: "m5", occurrence: 0 },
			offset: 0,
		});
	});
});

describe("findLabel", () => {
	const label = (state: Partial<FindState>) =>
		findLabel({ ...CLOSED_FIND, open: true, ...state });
	const result = selectFind(counts, { type: "index", index: 4 });

	it("says nothing before a query", () => {
		expect(label({ typed: "  " })).toEqual({ text: "", none: false });
	});

	it("says where the active match sits, or that there are none", () => {
		expect(label({ typed: "match" })).toEqual({ text: "Searching", none: false });
		expect(label({ typed: "match", result, index: 4 })).toEqual({
			text: "5 of 30",
			none: false,
		});
		expect(label({ typed: "x", result: selectFind([], { type: "index", index: 0 }) })).toEqual({
			text: "No matches",
			none: true,
		});
		expect(label({ typed: "match", result, status: "failed" })).toEqual({
			text: "Search failed",
			none: false,
		});
	});
});
