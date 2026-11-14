// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { mockSource } from "@/data/mock-source";
import type { TimelineFind, TimelineItem, TimelineUpdate } from "@maxhq/shared/bindings";
import type { DataSource } from "@maxhq/shared/data/source";
import { createFind, TYPE_MS } from "@maxhq/shared/find/store";
import { timelineItem } from "@maxhq/shared/timeline/build";
import { openTimeline } from "@maxhq/shared/timeline/store";

import type { JumpTarget } from "../jump";
import { FIND_ACTIVE_HIGHLIGHT, FIND_HIGHLIGHT } from "./highlights";
import { followFind } from "./use-find";

const say = (n: number, text: string): TimelineItem =>
	timelineItem(
		{ kind: "assistant_message", text, streaming: false },
		{ id: `m${n}`, ordinal: n }
	);

class FakeHighlight extends Set<Range> {
	constructor(...ranges: Range[]) {
		super(ranges);
	}
}

const scope = globalThis as unknown as { CSS?: object; Highlight?: unknown };
const realCSS = scope.CSS;

beforeEach(() => {
	vi.useFakeTimers({
		toFake: ["setTimeout", "clearTimeout", "requestAnimationFrame", "cancelAnimationFrame"],
	});
	Range.prototype.getBoundingClientRect = () => new DOMRect(0, 0, 0, 0);
	Object.defineProperty(globalThis, "CSS", {
		value: { highlights: new Map() },
		configurable: true,
		writable: true,
	});
	scope.Highlight = FakeHighlight;
});

afterEach(() => {
	vi.useRealTimers();
	Object.defineProperty(globalThis, "CSS", {
		value: realCSS,
		configurable: true,
		writable: true,
	});
	delete scope.Highlight;
	document.body.innerHTML = "";
});

/**
 * An agent's timeline, opened with only its newest page, and a scroller
 * that draws an item's row when the list is asked to scroll to it.
 */
const setup = () => {
	let push: (update: TimelineUpdate) => void = () => {};
	const loadOlder = vi.fn(async () => ({ status: "ok" as const, data: null }));
	const source = {
		...mockSource({ tick: false }),
		openTimeline: async (
			_agent: string,
			onUpdate: (update: TimelineUpdate, live: boolean) => void
		) => {
			push = (update) => onUpdate(update, false);
			return () => {};
		},
		loadOlder,
	} satisfies DataSource;
	const timeline = openTimeline(source, "a1", { schedule: (flush) => flush() });
	const scroller = document.createElement("div");
	document.body.append(scroller);
	const reveal = vi.fn((id: string) => {
		const item = timeline.store.getState().items.get(id);
		if (!item || item.kind !== "assistant_message") return false;
		if (!scroller.querySelector(`[data-quote-item="${id}"]`)) {
			const row = document.createElement("div");
			row.setAttribute("data-quote-item", id);
			row.innerHTML = `<div data-find="markdown"><p>${item.text}</p></div>`;
			scroller.append(row);
		}
		return true;
	});
	const target: JumpTarget = { timeline, scroller, reveal, hold: vi.fn() };
	const open = async () => {
		await vi.advanceTimersByTimeAsync(0);
		push({
			type: "reset",
			seq: 1,
			items: [say(5, "A match here."), say(6, "Nothing to see.")],
			has_older: true,
		});
	};
	return { target, open, push: (update: TimelineUpdate) => push(update), loadOlder };
};

// What maxd answers: two matches in m1, which only an older page has, and one in m5.
const answer: TimelineFind = {
	total: 3,
	active: 1,
	found: { item: "m1", occurrence: 1 },
	near: [
		{ item: "m1", first: 0, count: 2 },
		{ item: "m5", first: 2, count: 1 },
	],
};

const lit = (name: string) =>
	[...((CSS.highlights.get(name) as unknown as Set<Range> | undefined) ?? [])].map((range) => ({
		text: range.toString(),
		row: range.startContainer.parentElement
			?.closest("[data-quote-item]")
			?.getAttribute("data-quote-item"),
		at: range.startOffset,
	}));

const frames = (count = 4) => vi.advanceTimersByTime(count * 16);

describe("followFind", () => {
	it("loads older pages until it has the match's item, then lights the match", async () => {
		const { target, open, push, loadOlder } = setup();
		await open();
		const find = createFind(async () => answer);
		const stop = followFind(find, target);

		find.open();
		find.type("match");
		await vi.advanceTimersByTimeAsync(TYPE_MS);
		expect(loadOlder).toHaveBeenCalledTimes(1);
		push({
			type: "older",
			items: [say(1, "One match, then a second match.")],
			has_older: false,
		});
		frames();
		expect(target.reveal).toHaveBeenCalledWith("m1", "center");
		expect(lit(FIND_ACTIVE_HIGHLIGHT)).toEqual([{ text: "match", row: "m1", at: 25 }]);

		// The next match is in an item the answer named, so it goes there without asking.
		find.step(1);
		frames();
		expect(target.reveal).toHaveBeenCalledWith("m5", "center");
		expect(lit(FIND_ACTIVE_HIGHLIGHT)).toEqual([{ text: "match", row: "m5", at: 2 }]);
		expect(
			lit(FIND_HIGHLIGHT)
				.map((range) => range.row)
				.sort()
		).toEqual(["m1", "m1", "m5"]);

		find.close();
		expect(lit(FIND_HIGHLIGHT)).toEqual([]);
		expect(lit(FIND_ACTIVE_HIGHLIGHT)).toEqual([]);
		stop();
		find.dispose();
	});
});
