// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";

import { fakeTimeline, reply } from "../test/timeline";
import { EMPTY_TIMELINE } from "./apply";
import { loadItem, loadUntil } from "./seek";

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("loadItem", () => {
	it("waits for the timeline to open, then says it has the item", () => {
		const { timeline, push, loads } = fakeTimeline([]);
		const loaded = vi.fn();
		loadItem(timeline, "b", loaded, vi.fn());
		expect(loaded).not.toHaveBeenCalled();
		push({ type: "reset", seq: 1, items: [reply("a", 0), reply("b", 1)], has_older: true });
		expect(loaded).toHaveBeenCalledTimes(1);
		expect(loads()).toBe(0);
	});

	it("loads older pages until it has the item", async () => {
		const { timeline, push, loads } = fakeTimeline([[reply("b", 1)], [reply("a", 0)]]);
		push({ type: "reset", seq: 1, items: [reply("c", 2)], has_older: true });
		const loaded = vi.fn();
		loadItem(timeline, "a", loaded, vi.fn());
		await settle();
		expect(loads()).toBe(2);
		expect(loaded).toHaveBeenCalledTimes(1);
	});

	it("says why it stopped short", async () => {
		const { timeline, push } = fakeTimeline([[reply("a", 0)]]);
		push({ type: "reset", seq: 1, items: [reply("b", 1)], has_older: true });
		const failed = vi.fn();
		loadItem(timeline, "gone", vi.fn(), failed);
		await settle();
		expect(failed).toHaveBeenCalledWith("gone");

		const broken = fakeTimeline([]);
		broken.push({ type: "reset", seq: 1, items: [reply("b", 1)], has_older: true });
		const unloaded = vi.fn();
		loadItem(broken.timeline, "a", vi.fn(), unloaded);
		await settle();
		expect(unloaded).toHaveBeenCalledWith("unloaded");

		const shut = fakeTimeline([]);
		const unopened = vi.fn();
		loadItem(shut.timeline, "a", vi.fn(), unopened);
		shut.timeline.store.setState({ ...EMPTY_TIMELINE, status: "failed", error: "Gone" });
		expect(unopened).toHaveBeenCalledWith("unopened");
	});

	it("stops when asked", async () => {
		const { timeline, push, loads } = fakeTimeline([[reply("a", 0)]]);
		push({ type: "reset", seq: 1, items: [reply("b", 1)], has_older: true });
		const loaded = vi.fn();
		const stop = loadItem(timeline, "a", loaded, vi.fn());
		expect(loads()).toBe(1);
		stop();
		await settle();
		expect(loaded).not.toHaveBeenCalled();
	});
});

describe("loadUntil", () => {
	it("loads older pages until one has an item that fits", async () => {
		const { timeline, push, loads } = fakeTimeline([[reply("b", 1)], [reply("a", 0)]]);
		push({ type: "reset", seq: 1, items: [reply("c", 2)], has_older: true });
		const loaded = vi.fn();
		loadUntil(timeline, (state) => state.rows.find((id) => id < "c") ?? null, loaded, vi.fn());
		await settle();
		expect(loads()).toBe(1);
		expect(loaded).toHaveBeenCalledWith("b");
	});
});
