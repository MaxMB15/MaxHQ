// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Delta, Entity, Snapshot } from "../bindings";

import type { DataSource } from "../data/source";
import { agent, entities, NOW } from "../test/build";
import { remoteCalls } from "../test/remote";
import { connectView, createViewStore, retryDelay } from "./store";

const upsert = (seq: number, ...values: Entity[]): Delta => ({
	seq,
	changes: values.map((value) => ({ op: "upsert", value })),
});

const tagged = (id: string, title = id): Entity => entities({ agents: [agent({ id, title })] })[0]!;

const done = async () => ({ status: "ok", data: { status: "done" } }) as const;

/** A source whose snapshots and deltas a test hands out by hand. */
const fakeSource = () => {
	let emit: (delta: Delta) => void = () => {};
	const snapshots: Array<(s: Snapshot) => void> = [];
	const unsubscribe = vi.fn();
	const source: DataSource = {
		kind: "mock",
		snapshot: () => new Promise((resolve) => snapshots.push(resolve)),
		subscribe: async (onDelta) => {
			emit = onDelta;
			return unsubscribe;
		},
		onFocusAgent: async () => () => {},
		appInfo: async () => null,
		markSeen: async () => ({ status: "ok", data: null }),
		refreshUsage: async () => ({ status: "ok", data: null }),
		refreshLinks: async () => ({ status: "ok", data: null }),
		setShown: async () => {},
		openTimeline: async () => () => {},
		loadOlder: async () => ({ status: "ok", data: null }),
		openWorkflow: async () => () => {},
		nodeDetail: async () => ({ status: "ok", data: { lists: [] } }),
		startAgent: done,
		sendInput: done,
		cancelQueued: done,
		interrupt: done,
		setPermission: done,
		setModel: done,
		setEffort: done,
		stopAgent: done,
		setInbox: async (agents) => Promise.all(agents.map(done)),
		continueHere: done,
		takeOver: done,
		answer: done,
		stageAttachments: async () => [],
		stagePasted: done,
		setDeviceSettings: done,
		...remoteCalls(),
	};
	return {
		source,
		unsubscribe,
		emit: (delta: Delta) => emit(delta),
		/** Answers the oldest snapshot request. */
		answer: async (snapshot: Snapshot) => {
			await vi.waitFor(() => expect(snapshots.length).toBeGreaterThan(0));
			snapshots.shift()!(snapshot);
			await Promise.resolve();
		},
		requests: () => snapshots.length,
	};
};

const titles = (store: ReturnType<typeof createViewStore>) =>
	[...store.getState().view.tables.agent.values()].map((a) => a.title).sort();

describe("connectView", () => {
	it("applies deltas that arrive while the snapshot loads", async () => {
		const fake = fakeSource();
		const store = createViewStore();
		connectView(store, fake.source);
		await vi.waitFor(() => expect(fake.requests()).toBe(1));

		fake.emit(upsert(5, tagged("b")));
		fake.emit(upsert(6, tagged("c")));
		await fake.answer({ seq: 5, entities: [tagged("a"), tagged("b")] });

		await vi.waitFor(() => expect(store.getState().status.state).toBe("ready"));
		expect(store.getState().view.seq).toBe(6);
		expect(titles(store)).toEqual(["a", "b", "c"]);
	});

	it("applies live deltas and skips repeats", async () => {
		const fake = fakeSource();
		const store = createViewStore();
		connectView(store, fake.source);
		await fake.answer({ seq: 1, entities: [tagged("a")] });
		await vi.waitFor(() => expect(store.getState().status.state).toBe("ready"));

		fake.emit(upsert(2, tagged("a", "renamed")));
		fake.emit(upsert(2, tagged("a", "again")));
		expect(store.getState().view.seq).toBe(2);
		expect(titles(store)).toEqual(["renamed"]);
	});

	it("loads the snapshot again after a gap", async () => {
		const fake = fakeSource();
		const store = createViewStore();
		connectView(store, fake.source);
		await fake.answer({ seq: 1, entities: [tagged("a")] });
		await vi.waitFor(() => expect(store.getState().status.state).toBe("ready"));

		fake.emit(upsert(4, tagged("d")));
		await fake.answer({ seq: 4, entities: [tagged("a"), tagged("d")] });
		await vi.waitFor(() => expect(store.getState().view.seq).toBe(4));
		expect(titles(store)).toEqual(["a", "d"]);
	});

	it("stops listening when stopped", async () => {
		const fake = fakeSource();
		const store = createViewStore();
		const stop = connectView(store, fake.source);
		await fake.answer({ seq: 1, entities: [] });
		stop();
		expect(fake.unsubscribe).toHaveBeenCalledOnce();
		fake.emit(upsert(2, tagged("a")));
		expect(titles(store)).toEqual([]);
	});
});

describe("connectView after a failure", () => {
	beforeEach(() => {
		vi.useFakeTimers();
		vi.setSystemTime(NOW);
	});
	afterEach(() => vi.useRealTimers());

	/** A source whose snapshots fail `failing` times, then load. */
	const flaky = (failing: number) => {
		const snapshot = vi.fn(async (): Promise<Snapshot> => {
			if (snapshot.mock.calls.length <= failing) throw new Error("no maxd");
			return { seq: 1, entities: [tagged("a")] };
		});
		return { source: { ...fakeSource().source, snapshot }, snapshot };
	};

	it("waits 1 s, then twice as long each time, up to 30 s", () => {
		expect([1, 2, 3, 4, 5, 6, 7].map(retryDelay)).toEqual([
			1000, 2000, 4000, 8000, 16_000, 30_000, 30_000,
		]);
	});

	it("says when it tries again, and loads once a try works", async () => {
		const { source, snapshot } = flaky(2);
		const store = createViewStore();
		const stop = connectView(store, source);
		await vi.advanceTimersByTimeAsync(0);
		expect(store.getState().status).toEqual({
			state: "failed",
			message: "no maxd",
			retryAt: NOW + 1000,
		});

		await vi.advanceTimersByTimeAsync(999);
		expect(snapshot).toHaveBeenCalledTimes(1);
		await vi.advanceTimersByTimeAsync(1);
		expect(snapshot).toHaveBeenCalledTimes(2);
		expect(store.getState().status).toEqual({
			state: "failed",
			message: "no maxd",
			retryAt: NOW + 1000 + 2000,
		});

		await vi.advanceTimersByTimeAsync(2000);
		expect(store.getState().status).toEqual({ state: "ready" });
		expect(titles(store)).toEqual(["a"]);
		stop();
	});

	it("listens again when listening failed", async () => {
		const fake = fakeSource();
		const subscribe = vi
			.fn<DataSource["subscribe"]>()
			.mockRejectedValueOnce(new Error("no maxd"))
			.mockImplementation(fake.source.subscribe);
		const store = createViewStore();
		const stop = connectView(store, { ...fake.source, subscribe });
		await vi.advanceTimersByTimeAsync(0);
		expect(store.getState().status).toMatchObject({ state: "failed", retryAt: NOW + 1000 });
		expect(fake.requests()).toBe(0);

		await vi.advanceTimersByTimeAsync(1000);
		expect(subscribe).toHaveBeenCalledTimes(2);
		expect(fake.requests()).toBe(1);
		stop();
	});

	it("tries at once on Retry, and only once while that try runs", async () => {
		let answer: (snapshot: Snapshot) => void = () => {};
		const snapshot = vi
			.fn<DataSource["snapshot"]>()
			.mockRejectedValueOnce(new Error("no maxd"))
			.mockImplementation(() => new Promise((resolve) => (answer = resolve)));
		const store = createViewStore();
		const stop = connectView(store, { ...fakeSource().source, snapshot });
		await vi.advanceTimersByTimeAsync(0);

		store.getState().retry();
		store.getState().retry();
		expect(snapshot).toHaveBeenCalledTimes(2);
		expect(store.getState().status).toEqual({
			state: "failed",
			message: "no maxd",
			retryAt: null,
		});
		answer({ seq: 1, entities: [] });
		await vi.advanceTimersByTimeAsync(0);
		expect(store.getState().status).toEqual({ state: "ready" });
		stop();
	});

	it("stops trying once stopped", async () => {
		const { source, snapshot } = flaky(Infinity);
		const store = createViewStore();
		const stop = connectView(store, source);
		await vi.advanceTimersByTimeAsync(0);
		stop();
		store.getState().retry();
		await vi.advanceTimersByTimeAsync(60_000);
		expect(snapshot).toHaveBeenCalledTimes(1);
	});
});
