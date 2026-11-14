// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { mockSource } from "@/data/mock-source";
import type { ItemBody, ItemStatus, TimelineItem, TimelineUpdate } from "@maxhq/shared/bindings";
import type { DataSource } from "@maxhq/shared/data/source";
import { applyUpdate, EMPTY_TIMELINE } from "@maxhq/shared/timeline/apply";
import { timelineItem } from "@maxhq/shared/timeline/build";
import { openTimeline } from "@maxhq/shared/timeline/store";

import type { JumpTarget } from "./jump";
import {
	ASK_MS,
	askForRequest,
	clearRequests,
	goToRequest,
	takeRequest,
	waitingItem,
	watchRequests,
} from "./request";

const approval = (
	id: string,
	ordinal: number,
	attention: string,
	status: ItemStatus = "waiting",
	decided: Extract<ItemBody, { kind: "approval" }>["decided"] = null
): TimelineItem =>
	timelineItem(
		{
			kind: "approval",
			attention,
			approval_kind: "command",
			title: "Run `pnpm install`",
			detail: null,
			cwd: null,
			options: [],
			decided,
		},
		{ id, ordinal, status }
	);

const question = (id: string, ordinal: number, attention: string, answered = false) =>
	timelineItem(
		{
			kind: "question",
			attention,
			questions: [],
			answers: answered ? [] : null,
		},
		{ id, ordinal, status: answered ? "completed" : "waiting" }
	);

const say = (id: string, ordinal: number) =>
	timelineItem({ kind: "assistant_message", text: "Done", streaming: false }, { id, ordinal });

const stateOf = (items: TimelineItem[]) =>
	applyUpdate(EMPTY_TIMELINE, { type: "reset", seq: 1, items, has_older: false });

afterEach(() => {
	clearRequests();
	vi.useRealTimers();
	document.body.innerHTML = "";
});

describe("waitingItem", () => {
	it("finds the card for the request the agent waits on", () => {
		const state = stateOf([
			approval("i1", 1, "att-1"),
			question("i2", 2, "att-2"),
			say("i3", 3),
		]);
		expect(waitingItem(state, "att-1")).toBe("i1");
		expect(waitingItem(state, "att-2")).toBe("i2");
	});

	it("takes the newest card still waiting when the request isn't named", () => {
		const state = stateOf([
			approval("i1", 1, "att-1"),
			question("i2", 2, "att-2"),
			approval("i3", 3, "att-3", "completed", {
				answer: { type: "option", id: "allow" },
				by: null,
				at: 0,
			}),
		]);
		expect(waitingItem(state, null)).toBe("i2");
		expect(waitingItem(state, "att-gone")).toBe("i2");
	});

	it("skips cards already answered or closed", () => {
		const state = stateOf([
			approval("i1", 1, "att-1", "failed"),
			question("i2", 2, "att-2", true),
			say("i3", 3),
		]);
		expect(waitingItem(state, null)).toBeNull();
	});
});

describe("asks", () => {
	it("tells the agent's list, which takes the ask once", () => {
		const heard = vi.fn();
		const unwatch = watchRequests("a1", heard);
		askForRequest("a1", "att-1");
		expect(heard).toHaveBeenCalledOnce();
		expect(takeRequest("a1")).toMatchObject({ attention: "att-1" });
		expect(takeRequest("a1")).toBeNull();
		unwatch();
		askForRequest("a1", null);
		expect(heard).toHaveBeenCalledOnce();
	});

	it("drops an ask that waited too long for its list", () => {
		vi.useFakeTimers();
		askForRequest("a1", "att-1");
		vi.advanceTimersByTime(ASK_MS + 1);
		expect(takeRequest("a1")).toBeNull();
	});
});

describe("goToRequest", () => {
	beforeEach(() => {
		vi.useFakeTimers({
			toFake: ["setTimeout", "clearTimeout", "requestAnimationFrame", "cancelAnimationFrame"],
		});
	});

	// An opened timeline and a scroller that draws a row with its card when the
	// list is asked to scroll to it, the way the virtualized list does.
	const setup = () => {
		let push: (update: TimelineUpdate) => void = () => {};
		const source = {
			...mockSource({ tick: false }),
			openTimeline: async (
				_agent: string,
				onUpdate: (update: TimelineUpdate, live: boolean) => void
			) => {
				push = (update) => onUpdate(update, false);
				return () => {};
			},
		} satisfies DataSource;
		const timeline = openTimeline(source, "a1", { schedule: (flush) => flush() });
		const scroller = document.createElement("div");
		document.body.append(scroller);
		const reveal = vi.fn((id: string) => {
			if (!scroller.querySelector(`[data-quote-item="${id}"]`)) {
				const row = document.createElement("div");
				row.setAttribute("data-quote-item", id);
				row.innerHTML = `<section tabindex="-1" data-waiting-card>Allow?</section>`;
				scroller.append(row);
			}
			return true;
		});
		const hold = vi.fn();
		const target: JumpTarget = { timeline, scroller, reveal, hold };
		const open = async (items: TimelineItem[]) => {
			await vi.advanceTimersByTimeAsync(0);
			push({ type: "reset", seq: 1, items, has_older: false });
		};
		return { target, open, push: (update: TimelineUpdate) => push(update) };
	};

	const frames = (count = 4) => vi.advanceTimersByTime(count * 16);

	it("waits for the timeline, then scrolls to the card and focuses it", async () => {
		const { target, open } = setup();
		goToRequest(target, "att-1");
		await open([say("i0", 0), approval("i1", 1, "att-1")]);
		frames();
		expect(target.reveal).toHaveBeenCalledWith("i1", "center");
		vi.advanceTimersByTime(0);
		expect(document.activeElement).toBe(
			target.scroller.querySelector('[data-quote-item="i1"] [data-waiting-card]')
		);
	});

	it("waits for the card when the timeline doesn't have it yet", async () => {
		const { target, open, push } = setup();
		await open([say("i0", 0)]);
		goToRequest(target, "att-1");
		frames();
		expect(target.reveal).not.toHaveBeenCalled();
		push({
			type: "events",
			seq: 2,
			events: [{ type: "upsert", item: question("i1", 1, "att-1") }],
		});
		frames();
		vi.advanceTimersByTime(0);
		expect(document.activeElement?.hasAttribute("data-waiting-card")).toBe(true);
	});

	it("gives up when no card comes", async () => {
		const { target, open, push } = setup();
		await open([say("i0", 0)]);
		goToRequest(target, null);
		vi.advanceTimersByTime(ASK_MS + 1);
		push({
			type: "events",
			seq: 2,
			events: [{ type: "upsert", item: question("i1", 1, "att-1") }],
		});
		frames();
		expect(target.reveal).not.toHaveBeenCalled();
	});

	it("stops when asked to", async () => {
		const { target, open } = setup();
		const stop = goToRequest(target, "att-1");
		stop();
		await open([approval("i1", 1, "att-1")]);
		frames();
		expect(target.reveal).not.toHaveBeenCalled();
	});
});
