// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { FindAt, TimelineItem } from "@maxhq/shared/bindings";
import { countOccurrences, selectFind } from "@maxhq/shared/find/search";
import { findLabel, TYPE_MS } from "@maxhq/shared/find/store";
import { timelineItem } from "@maxhq/shared/timeline/build";
import { fakeTimeline, reply } from "@maxhq/shared/test/timeline";

import { createFeedFind, type FeedFind } from "./find";

const settle = () => vi.advanceTimersByTimeAsync(0);

// The finds the tests started, to stop after each one.
const stops: (() => void)[] = [];

// The text find reads in each item, as maxd reads it.
const textOf = (item: TimelineItem) =>
	item.kind === "assistant_message" || item.kind === "reasoning" ? item.text : "";

/**
 * An agent whose feed has its newest `loaded` items, with the rest on
 * older pages of one item each, and a source that searches every item, as
 * maxd does, keeping the queries it's asked.
 */
const setup = (items: TimelineItem[], loaded = items.length) => {
	const older = items.slice(0, items.length - loaded);
	const { timeline, push, loads } = fakeTimeline(older.reverse().map((item) => [item]));
	push({
		type: "reset",
		seq: 1,
		items: items.slice(items.length - loaded),
		has_older: items.length > loaded,
	});
	const asked: string[] = [];
	const source = {
		findInTimeline: async (_agent: string, query: string, at: FindAt) => {
			asked.push(query);
			const counts = items.map((item) => ({
				item: item.id,
				count: countOccurrences(textOf(item), query),
			}));
			return selectFind(counts, at);
		},
	};
	const feed = createFeedFind(source, "a1", timeline);
	stops.push(feed.follow());
	return { feed, timeline, loads, asked };
};

const ITEMS = [
	reply("a", 0, "Fix the build"),
	reply("b", 1, "Nothing here"),
	reply("c", 2, "Fix it, then fix the test"),
	reply("d", 3, "Done"),
];

describe("find in the feed", () => {
	let feed: FeedFind;

	beforeEach(() => {
		vi.useFakeTimers();
	});

	afterEach(() => {
		for (const stop of stops.splice(0)) stop();
		vi.useRealTimers();
	});

	const search = async (typed: string) => {
		feed.find.open();
		feed.find.type(typed);
		await vi.advanceTimersByTimeAsync(TYPE_MS);
		await settle();
	};
	const state = () => feed.find.store.getState();
	const goal = () => feed.goal.getState();

	it("opens the field from the More sheet's Find", () => {
		({ feed } = setup(ITEMS));
		feed.find.open();
		expect(state()).toMatchObject({ open: true, focus: 1 });
		expect(findLabel(state()).text).toBe("");
	});

	it("searches as you type and counts the matches", async () => {
		const made = setup(ITEMS);
		feed = made.feed;
		feed.find.open();
		feed.find.type("f");
		feed.find.type("fix");
		expect(findLabel(state()).text).toBe("Searching");
		await vi.advanceTimersByTimeAsync(TYPE_MS);
		await settle();
		expect(made.asked).toEqual(["fix"]);
		expect(findLabel(state()).text).toBe("1 of 3");
		expect(goal()).toEqual({ row: "a", turn: 1 });

		feed.find.type("missing");
		await vi.advanceTimersByTimeAsync(TYPE_MS);
		await settle();
		expect(findLabel(state())).toEqual({ text: "No matches", none: true });
		expect(goal().row).toBeNull();
	});

	it("moves to the next and previous match, wrapping round", async () => {
		({ feed } = setup(ITEMS));
		await search("fix");
		feed.find.step(1);
		expect(findLabel(state()).text).toBe("2 of 3");
		expect(goal()).toEqual({ row: "c", turn: 2 });
		feed.find.step(1);
		expect(findLabel(state()).text).toBe("3 of 3");
		expect(goal()).toEqual({ row: "c", turn: 3 });
		feed.find.step(1);
		expect(findLabel(state()).text).toBe("1 of 3");
		expect(goal()).toEqual({ row: "a", turn: 4 });
		feed.find.step(-1);
		expect(findLabel(state()).text).toBe("3 of 3");
		expect(goal().row).toBe("c");
	});

	it("loads older pages before it goes to a match the feed doesn't have", async () => {
		const made = setup(ITEMS, 2);
		feed = made.feed;
		// The older pages each load had asked for when the feed went to a row.
		const went: number[] = [];
		feed.goal.subscribe(({ row }) => {
			if (row) went.push(made.loads());
		});
		await search("fix");
		expect(went).toEqual([2]);
		expect(made.timeline.store.getState().rows).toEqual(["a", "b", "c", "d"]);
		expect(goal()).toEqual({ row: "a", turn: 1 });
	});

	it("marks a reasoning step's own row for a match in it", async () => {
		const thought = timelineItem(
			{ kind: "reasoning", text: "Ponder the build", streaming: false },
			{ id: "r", ordinal: 1 }
		);
		({ feed } = setup([reply("a", 0), thought, reply("b", 2)]));
		await search("ponder");
		expect(state().match).toEqual({ item: "r", occurrence: 0 });
		expect(goal().row).toBe("r");
	});

	it("clears the mark when it closes and leaves the feed loaded", async () => {
		const made = setup(ITEMS, 2);
		feed = made.feed;
		await search("fix");
		expect(goal().row).toBe("a");
		feed.find.close();
		expect(state()).toMatchObject({ open: false, typed: "", result: null, match: null });
		expect(goal().row).toBeNull();
		expect(made.timeline.store.getState().rows).toHaveLength(4);
	});

	it("stops loading when it closes on the way to a match", async () => {
		const made = setup(ITEMS, 2);
		feed = made.feed;
		// Closes once the first older page is in.
		made.timeline.store.subscribe(({ rows }) => {
			if (rows.length === 3) feed.find.close();
		});
		await search("fix");
		expect(made.loads()).toBe(1);
		expect(goal()).toEqual({ row: null, turn: 0 });
	});
});
