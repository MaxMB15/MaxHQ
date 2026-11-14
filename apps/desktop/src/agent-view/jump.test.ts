// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { mockSource } from "@/data/mock-source";
import type { TimelineItem, TimelineUpdate } from "@maxhq/shared/bindings";
import type { DataSource } from "@maxhq/shared/data/source";
import type { QuoteRef } from "@maxhq/shared/pills/draft";
import { timelineItem } from "@maxhq/shared/timeline/build";
import { openTimeline } from "@maxhq/shared/timeline/store";

import {
	createQuoteJumps,
	FLASH_ATTRIBUTE,
	FLASH_MS,
	JUMP_HIGHLIGHT,
	type JumpApp,
	type JumpTarget,
} from "./jump";

const say = (n: number, text = `Message ${n}`): TimelineItem =>
	timelineItem(
		{ kind: "assistant_message", text, streaming: false },
		{ id: `m${n}`, ordinal: n }
	);

const quote: QuoteRef = {
	type: "quote",
	agent: "a1",
	agent_title: "Fix the build",
	item: "m1",
	text: "the part you want",
	position: 0,
	source: "reply",
	from: null,
};

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
 * An agent's timeline, opened, with a scroller that draws each item's row
 * when the list is asked to scroll to it, the way the virtualized list does.
 */
const setup = (items: TimelineItem[], hasOlder = false) => {
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
			row.innerHTML = `<p>${item.text}</p>`;
			scroller.append(row);
		}
		return true;
	});
	const target: JumpTarget = { timeline, scroller, reveal, hold: () => {} };
	const app = {
		open: vi.fn<JumpApp["open"]>(),
		titleOf: vi.fn<JumpApp["titleOf"]>(() => "Fix the build"),
		fail: vi.fn<JumpApp["fail"]>(),
	};
	const open = async () => {
		await vi.advanceTimersByTimeAsync(0);
		push({ type: "reset", seq: 1, items, has_older: hasOlder });
	};
	return {
		timeline,
		target,
		app,
		push: (update: TimelineUpdate) => push(update),
		open,
		loadOlder,
	};
};

const lit = () =>
	[...((CSS.highlights.get(JUMP_HIGHLIGHT) as unknown as Set<Range> | undefined) ?? [])].map(
		(range) => range.toString()
	);

const frames = (count = 4) => vi.advanceTimersByTime(count * 16);

describe("createQuoteJumps", () => {
	it("opens the agent, scrolls to the row and lights the passage for a moment", async () => {
		const { target, app, open } = setup([say(1, "The second one, with the part you want.")]);
		const jumps = createQuoteJumps();
		jumps.register("a1", target);
		await open();
		jumps.jump(quote, app);
		expect(app.open).toHaveBeenCalledWith("a1");
		frames();
		expect(target.reveal).toHaveBeenCalledWith("m1", "center");
		expect(lit()).toEqual(["the part you want"]);
		vi.advanceTimersByTime(FLASH_MS);
		expect(lit()).toEqual([]);
		expect(app.fail).not.toHaveBeenCalled();
	});

	it("lights the whole row when the passage is gone", async () => {
		const { target, app, open } = setup([say(1, "Rewritten since.")]);
		const jumps = createQuoteJumps();
		jumps.register("a1", target);
		await open();
		jumps.jump(quote, app);
		frames();
		const row = target.scroller.querySelector("[data-quote-item=m1]")!;
		expect(row.hasAttribute(FLASH_ATTRIBUTE)).toBe(true);
		expect(lit()).toEqual([]);
		vi.advanceTimersByTime(FLASH_MS);
		expect(row.hasAttribute(FLASH_ATTRIBUTE)).toBe(false);
		expect(app.fail).not.toHaveBeenCalled();
	});

	it("waits for the list to mount and the timeline to open", async () => {
		const { target, app, open } = setup([say(1, "The second one, with the part you want.")]);
		const jumps = createQuoteJumps();
		jumps.jump(quote, app);
		jumps.register("a1", target);
		frames();
		expect(target.reveal).not.toHaveBeenCalled();
		await open();
		frames();
		expect(lit()).toEqual(["the part you want"]);
	});

	it("loads older pages until it has the item", async () => {
		const { target, app, open, push, loadOlder } = setup([say(5), say(6)], true);
		const jumps = createQuoteJumps();
		jumps.register("a1", target);
		await open();
		jumps.jump(quote, app);
		expect(loadOlder).toHaveBeenCalledTimes(1);
		push({ type: "older", items: [say(3), say(4)], has_older: true });
		expect(loadOlder).toHaveBeenCalledTimes(2);
		push({
			type: "older",
			items: [say(1, "The second one, with the part you want.")],
			has_older: false,
		});
		frames();
		expect(lit()).toEqual(["the part you want"]);
	});

	it("says so when the item is gone", async () => {
		const { target, app, open } = setup([say(5)]);
		const jumps = createQuoteJumps();
		jumps.register("a1", target);
		await open();
		jumps.jump(quote, app);
		expect(app.fail).toHaveBeenCalledWith("The quoted message is no longer in Fix the build.");
		expect(target.reveal).not.toHaveBeenCalled();
	});

	it("says so when an older page doesn't load", async () => {
		const { target, app, open, loadOlder } = setup([say(5)], true);
		loadOlder.mockResolvedValueOnce({
			status: "error",
			error: { kind: "failed", message: "offline" },
		} as never);
		const jumps = createQuoteJumps();
		jumps.register("a1", target);
		await open();
		jumps.jump(quote, app);
		await vi.advanceTimersByTimeAsync(0);
		expect(app.fail).toHaveBeenCalledWith(
			"Couldn't load earlier items in Fix the build to find the quote."
		);
	});

	it("says so when the agent is gone, and opens nothing", () => {
		const { app } = setup([]);
		app.titleOf.mockReturnValue(null);
		createQuoteJumps().jump(quote, app);
		expect(app.fail).toHaveBeenCalledWith(
			"Fix the build is gone, so the quote can't be shown."
		);
		expect(app.open).not.toHaveBeenCalled();
	});

	it("drops a jump when a newer one starts or its list goes", async () => {
		const { target, app, open } = setup([say(1, "The second one, with the part you want.")]);
		const jumps = createQuoteJumps();
		jumps.jump(quote, app);
		jumps.jump({ ...quote, agent: "a2" }, app);
		const undo = jumps.register("a1", target);
		await open();
		frames();
		expect(target.reveal).not.toHaveBeenCalled();

		jumps.jump(quote, app);
		undo();
		frames();
		expect(lit()).toEqual([]);
	});
});
