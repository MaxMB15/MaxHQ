// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";
import type { TimelineItem, TimelineUpdate } from "@maxhq/shared/bindings";

import { mockSource } from "@/data/mock-source";
import type { DataSource } from "@maxhq/shared/data/source";
import { applyUpdate, EMPTY_TIMELINE, type TimelineState } from "@maxhq/shared/timeline/apply";
import { timelineItem } from "@maxhq/shared/timeline/build";
import { leaseTimeline, sharedTimeline } from "@maxhq/shared/timeline/registry";
import { openTimeline } from "@maxhq/shared/timeline/store";

const say = (id: string, ordinal: number, text = id): TimelineItem =>
	timelineItem({ kind: "assistant_message", text, streaming: false }, { id, ordinal });

const sent = (id: string, ordinal: number, delivery: "sent" | "queued" | "cancelled") =>
	timelineItem({ kind: "user_message", text: id, attachments: [], delivery }, { id, ordinal });

const reset = (items: TimelineItem[], seq = 1, has_older = false): TimelineUpdate => ({
	type: "reset",
	seq,
	items,
	has_older,
});

const textOf = (state: TimelineState, id: string) => {
	const item = state.items.get(id);
	return item && "text" in item ? item.text : undefined;
};

const run = (...updates: TimelineUpdate[]) => updates.reduce(applyUpdate, EMPTY_TIMELINE);

describe("applyUpdate", () => {
	it("a reset replaces everything and sorts by ordinal", () => {
		const state = run(reset([say("a", 0)]), reset([say("c", 7), say("b", 3)], 4, true));
		expect(state.status).toBe("ready");
		expect(state.order).toEqual(["b", "c"]);
		expect(state.items.has("a")).toBe(false);
		expect(state.seq).toBe(4);
		expect(state.hasOlder).toBe(true);
	});

	it("events apply in order", () => {
		const state = run(reset([say("a", 0), say("b", 1)]), {
			type: "events",
			seq: 2,
			events: [
				{ type: "upsert", item: say("c", 2, "hello") },
				{ type: "append", id: "c", field: "text", text: " there" },
				{ type: "remove", id: "a" },
				{ type: "upsert", item: say("b", 1, "replaced") },
			],
		});
		expect(state.order).toEqual(["b", "c"]);
		expect(textOf(state, "b")).toBe("replaced");
		expect(textOf(state, "c")).toBe("hello there");
		expect(state.seq).toBe(2);
	});

	it("a burst of removes keeps the rest in order, around upserts that move items", () => {
		const first = run(reset(Array.from({ length: 10 }, (_, n) => say(`i${n}`, n))));
		const state = applyUpdate(first, {
			type: "events",
			seq: 2,
			events: [
				{ type: "remove", id: "i0" },
				{ type: "remove", id: "i1" },
				{ type: "remove", id: "i2" },
				{ type: "upsert", item: say("i3", 20) },
				{ type: "remove", id: "i4" },
				{ type: "remove", id: "unknown" },
				{ type: "upsert", item: say("i4", 4, "back") },
				{ type: "remove", id: "i9" },
			],
		});
		expect(state.order).toEqual(["i4", "i5", "i6", "i7", "i8", "i3"]);
		expect(state.rows).toEqual(state.order);
		expect(state.items.size).toBe(6);
		expect(textOf(state, "i4")).toBe("back");
		expect(first.order).toHaveLength(10);
	});

	it("appends join into one copy, and a later upsert wins over them", () => {
		const first = run(reset([say("a", 0, "")]));
		const state = applyUpdate(first, {
			type: "events",
			seq: 2,
			events: [
				{ type: "append", id: "a", field: "text", text: "one " },
				{ type: "append", id: "a", field: "text", text: "two " },
				{ type: "append", id: "a", field: "text", text: "three" },
			],
		});
		expect(textOf(state, "a")).toBe("one two three");
		// Only the item changed, so the lists keep their identity and rows don't re-render.
		expect(state.order).toBe(first.order);
		expect(state.rows).toBe(first.rows);

		const replaced = applyUpdate(state, {
			type: "events",
			seq: 3,
			events: [
				{ type: "append", id: "a", field: "text", text: " lost" },
				{ type: "upsert", item: say("a", 0, "final") },
			],
		});
		expect(textOf(replaced, "a")).toBe("final");
	});

	it("a batch leaves the state before it as it was and shares the items it didn't touch", () => {
		const first = run(reset(Array.from({ length: 100 }, (_, n) => say(`i${n}`, n, ""))));
		let state = first;
		for (let seq = 2; seq < 80; seq++) {
			const before = state;
			state = applyUpdate(state, {
				type: "events",
				seq,
				events: [
					{ type: "append", id: "i99", field: "text", text: "x" },
					{ type: "upsert", item: say(`new${seq}`, 100 + seq) },
				],
			});
			expect(before.items.has(`new${seq}`)).toBe(false);
			expect(textOf(before, "i99")).toBe("x".repeat(seq - 2));
		}
		expect(textOf(state, "i99")).toBe("x".repeat(78));
		expect(state.items.size).toBe(178);
		expect(state.items.get("i0")).toBe(first.items.get("i0"));
		expect([...state.items.keys()]).toEqual(state.order);
	});

	it("appends output to a command and markdown to a plan", () => {
		const command = timelineItem(
			{
				kind: "command",
				command: "pnpm test",
				cwd: null,
				exit_code: null,
				output: "a\n",
				output_truncated: false,
				background: false,
			},
			{ id: "cmd", ordinal: 0 }
		);
		const plan = timelineItem(
			{ kind: "plan", markdown: "1. ", streaming: true },
			{ id: "p", ordinal: 1 }
		);
		const state = run(reset([command, plan]), {
			type: "events",
			seq: 2,
			events: [
				{ type: "append", id: "cmd", field: "output", text: "b\n" },
				{ type: "append", id: "p", field: "text", text: "Read the docs" },
			],
		});
		const after = state.items.get("cmd");
		expect(after?.kind === "command" && after.output).toBe("a\nb\n");
		const planAfter = state.items.get("p");
		expect(planAfter?.kind === "plan" && planAfter.markdown).toBe("1. Read the docs");
	});

	it("drops events older than what it has", () => {
		const state = run(reset([say("a", 0)], 5), {
			type: "events",
			seq: 5,
			events: [{ type: "remove", id: "a" }],
		});
		expect(state.order).toEqual(["a"]);
	});

	it("ignores events before the first reset", () => {
		const state = run({
			type: "events",
			seq: 2,
			events: [{ type: "upsert", item: say("a", 0) }],
		});
		expect(state).toBe(EMPTY_TIMELINE);
	});

	it("an older page goes in front and skips items it already has", () => {
		const state = run(reset([say("c", 10), say("d", 11)], 1, true), {
			type: "older",
			items: [say("a", 2), say("b", 5), say("c", 10)],
			has_older: false,
		});
		expect(state.order).toEqual(["a", "b", "c", "d"]);
		expect(state.hasOlder).toBe(false);
		expect(state.loadingOlder).toBe(false);
	});

	it("an upsert out of order lands by ordinal", () => {
		const state = run(reset([say("a", 0), say("c", 4)]), {
			type: "events",
			seq: 2,
			events: [{ type: "upsert", item: say("b", 2) }],
		});
		expect(state.order).toEqual(["a", "b", "c"]);
	});

	it("keeps queued and cancelled messages out of the rows and lists the queue", () => {
		const start = run(reset([say("a", 0), sent("q", 1, "queued"), sent("x", 2, "cancelled")]));
		expect(start.rows).toEqual(["a"]);
		expect(start.queued.map((i) => i.id)).toEqual(["q"]);

		const delivered = applyUpdate(start, {
			type: "events",
			seq: 2,
			events: [{ type: "upsert", item: sent("q", 1, "sent") }],
		});
		expect(delivered.rows).toEqual(["a", "q"]);
		expect(delivered.queued).toEqual([]);
	});
});

const fakeSource = () => {
	let push: (update: TimelineUpdate, live: boolean) => void = () => {};
	const loadOlder = vi.fn(async () => ({ status: "ok" as const, data: null }));
	const close = vi.fn();
	const source = {
		...mockSource({ tick: false }),
		openTimeline: async (
			_agent: string,
			onUpdate: (update: TimelineUpdate, live: boolean) => void
		) => {
			push = onUpdate;
			return close;
		},
		loadOlder,
	} satisfies DataSource;
	return { source, push: (u: TimelineUpdate, live = false) => push(u, live), loadOlder, close };
};

// A source that gives each open a close of its own, and refuses every open
// after the first when `refuse` is set.
const reopenable = (refuse = false) => {
	const fake = fakeSource();
	const closes: ReturnType<typeof vi.fn>[] = [];
	const source = {
		...fake.source,
		openTimeline: async (
			agent: string,
			onUpdate: (update: TimelineUpdate, live: boolean) => void
		) => {
			if (refuse && closes.length > 0) throw new Error("gone");
			await fake.source.openTimeline(agent, onUpdate);
			const close = vi.fn();
			closes.push(close);
			return close;
		},
	} satisfies DataSource;
	return { source, push: fake.push, closes };
};

describe("openTimeline", () => {
	it("applies a reset at once and batches events into one frame", async () => {
		const { source, push } = fakeSource();
		const frames: (() => void)[] = [];
		const timeline = openTimeline(source, "a1", { schedule: (flush) => frames.push(flush) });
		await Promise.resolve();
		push(reset([say("a", 0, "")]));
		expect(timeline.store.getState().order).toEqual(["a"]);

		const seen = vi.fn();
		timeline.store.subscribe(seen);
		push({
			type: "events",
			seq: 2,
			events: [{ type: "append", id: "a", field: "text", text: "x" }],
		});
		push({
			type: "events",
			seq: 3,
			events: [{ type: "append", id: "a", field: "text", text: "y" }],
		});
		expect(frames).toHaveLength(1);
		expect(textOf(timeline.store.getState(), "a")).toBe("");

		frames[0]?.();
		expect(textOf(timeline.store.getState(), "a")).toBe("xy");
		expect(seen).toHaveBeenCalledTimes(1);
	});

	it("tells of new items only when they arrive live", async () => {
		const { source, push } = fakeSource();
		const timeline = openTimeline(source, "a1", { schedule: (flush) => flush() });
		await Promise.resolve();
		const arrived = vi.fn();
		timeline.onArrive((items) => arrived(items.map((item) => item.id)));
		push(reset([say("a", 0)]));
		push({ type: "events", seq: 2, events: [{ type: "upsert", item: say("b", 1) }] }, false);
		push({ type: "older", items: [], has_older: false });
		expect(arrived).not.toHaveBeenCalled();

		push({ type: "events", seq: 3, events: [{ type: "upsert", item: say("c", 2) }] }, true);
		// A change to an item already there isn't a new one.
		push(
			{ type: "events", seq: 4, events: [{ type: "upsert", item: say("c", 2, "x") }] },
			true
		);
		expect(arrived.mock.calls).toEqual([[["c"]]]);
	});

	it("an older page flushes waiting events first, so order holds", async () => {
		const { source, push } = fakeSource();
		const frames: (() => void)[] = [];
		const timeline = openTimeline(source, "a1", { schedule: (flush) => frames.push(flush) });
		await Promise.resolve();
		push(reset([say("b", 5)], 1, true));
		push({ type: "events", seq: 2, events: [{ type: "upsert", item: say("c", 6) }] });
		push({ type: "older", items: [say("a", 1)], has_older: false });
		expect(timeline.store.getState().order).toEqual(["a", "b", "c"]);
	});

	it("asks for the page before the first item, once at a time", async () => {
		const { source, push, loadOlder } = fakeSource();
		const timeline = openTimeline(source, "a1", { schedule: (flush) => flush() });
		await Promise.resolve();
		push(reset([say("b", 40), say("c", 41)], 1, true));
		timeline.loadOlder();
		timeline.loadOlder();
		expect(loadOlder).toHaveBeenCalledTimes(1);
		expect(loadOlder).toHaveBeenCalledWith("a1", 40);
		expect(timeline.store.getState().loadingOlder).toBe(true);

		push({ type: "older", items: [say("a", 2)], has_older: false });
		timeline.loadOlder();
		expect(loadOlder).toHaveBeenCalledTimes(1);
	});

	it("a failed page clears the flag and keeps the reason", async () => {
		const { source, push, loadOlder } = fakeSource();
		loadOlder.mockResolvedValueOnce({
			status: "error",
			error: { kind: "not_connected", device: "devbox" },
		} as never);
		const timeline = openTimeline(source, "a1", { schedule: (flush) => flush() });
		await Promise.resolve();
		push(reset([say("b", 40)], 1, true));
		timeline.loadOlder();
		await vi.waitFor(() => expect(timeline.store.getState().loadingOlder).toBe(false));
		expect(timeline.store.getState().error).toBe("devbox isn't connected.");
	});

	it("stops listening when closed", async () => {
		const { source, close } = fakeSource();
		const timeline = openTimeline(source, "a1");
		await Promise.resolve();
		timeline.close();
		expect(close).toHaveBeenCalled();
	});

	it("a reload opens again, lets the first open go once it's in, and takes the new reset", async () => {
		const { source, push, closes } = reopenable();
		const timeline = openTimeline(source, "a1", { schedule: (flush) => flush() });
		await Promise.resolve();
		push(reset([say("b", 5)], 1, true));
		push({ type: "older", items: [say("a", 1)], has_older: false });

		await timeline.reload();
		expect(closes).toHaveLength(2);
		expect(closes[0]).toHaveBeenCalledTimes(1);
		expect(closes[1]).not.toHaveBeenCalled();
		push(reset([say("b", 5), say("c", 6)], 4, true));
		expect(timeline.store.getState()).toMatchObject({ order: ["b", "c"], hasOlder: true });

		timeline.close();
		expect(closes[1]).toHaveBeenCalledTimes(1);
	});

	it("a reload that fails keeps the open and what it shows", async () => {
		const { source, push, closes } = reopenable(true);
		const timeline = openTimeline(source, "a1", { schedule: (flush) => flush() });
		await Promise.resolve();
		push(reset([say("b", 5)]));

		await timeline.reload();
		expect(closes[0]).not.toHaveBeenCalled();
		expect(timeline.store.getState()).toMatchObject({ status: "ready", error: null });
	});
});

describe("leaseTimeline", () => {
	it("shares one timeline and closes it a while after the last view goes", async () => {
		vi.useFakeTimers();
		try {
			const { source, close } = fakeSource();
			const one = leaseTimeline(source, "a1", 1000);
			const two = leaseTimeline(source, "a1", 1000);
			expect(two.timeline).toBe(one.timeline);
			await Promise.resolve();
			one.release();
			two.release();
			vi.advanceTimersByTime(500);
			const three = leaseTimeline(source, "a1", 1000);
			expect(three.timeline).toBe(one.timeline);
			three.release();
			vi.advanceTimersByTime(1000);
			expect(close).toHaveBeenCalledTimes(1);
			expect(leaseTimeline(source, "a1", 1000).timeline).not.toBe(one.timeline);
		} finally {
			vi.useRealTimers();
		}
	});

	it("gives a render the timeline a lease shares, and closes one nobody leases", async () => {
		vi.useFakeTimers();
		try {
			const { source, close } = fakeSource();
			const seen = sharedTimeline(source, "a1", 1000);
			expect(sharedTimeline(source, "a1", 1000)).toBe(seen);
			const lease = leaseTimeline(source, "a1", 1000);
			expect(lease.timeline).toBe(seen);
			await Promise.resolve();
			vi.advanceTimersByTime(2000);
			expect(close).not.toHaveBeenCalled();
			lease.release();
			vi.advanceTimersByTime(1000);
			expect(close).toHaveBeenCalledTimes(1);

			const unused = sharedTimeline(source, "a2", 1000);
			await Promise.resolve();
			vi.advanceTimersByTime(1000);
			expect(close).toHaveBeenCalledTimes(2);
			expect(sharedTimeline(source, "a2", 1000)).not.toBe(unused);
		} finally {
			vi.useRealTimers();
		}
	});
});
