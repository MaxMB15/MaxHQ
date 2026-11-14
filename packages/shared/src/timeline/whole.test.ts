// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { fakeTimeline, reply as say } from "../test/timeline";
import { EMPTY_TIMELINE } from "./apply";
import { loadAll } from "./whole";

describe("loadAll", () => {
	it("waits for the first page, then loads every older one", async () => {
		const { timeline, push, loads } = fakeTimeline([[say("b", 1)], [say("a", 0)]]);
		const whole = loadAll(timeline);
		push({ type: "reset", seq: 1, items: [say("c", 2)], has_older: true });
		const state = await whole;
		expect(state.order).toEqual(["a", "b", "c"]);
		expect(loads()).toBe(2);
	});

	it("resolves at once when there's nothing older", async () => {
		const { timeline, push, loads } = fakeTimeline([]);
		push({ type: "reset", seq: 1, items: [say("a", 0)], has_older: false });
		await expect(loadAll(timeline)).resolves.toMatchObject({ order: ["a"] });
		expect(loads()).toBe(0);
	});

	it("rejects with the reason a page failed", async () => {
		const { timeline, push } = fakeTimeline([], "devbox isn't connected.");
		push({ type: "reset", seq: 1, items: [say("b", 1)], has_older: true });
		await expect(loadAll(timeline)).rejects.toThrow("devbox isn't connected.");
	});

	it("doesn't count an error from before it asked", async () => {
		const { timeline, push } = fakeTimeline([[say("a", 0)]]);
		push({ type: "reset", seq: 1, items: [say("b", 1)], has_older: true });
		timeline.store.setState({ error: "An old failure" });
		await expect(loadAll(timeline)).resolves.toMatchObject({ order: ["a", "b"] });
	});

	it("rejects when the timeline doesn't open", async () => {
		const { timeline } = fakeTimeline([]);
		const whole = loadAll(timeline);
		timeline.store.setState({ ...EMPTY_TIMELINE, status: "failed", error: "Gone" });
		await expect(whole).rejects.toThrow("Gone");
	});
});
