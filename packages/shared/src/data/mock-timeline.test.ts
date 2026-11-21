// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it, vi } from "vitest";
import type { TimelineItem, TimelineUpdate } from "../bindings";

import { ITEM_KINDS } from "../timeline/build";
import { createMockTimelines, LONG_HISTORY, PAGE, STREAMING } from "./mock-timeline";

const NOW = Date.UTC(2026, 9, 25, 14, 30);

const firstPage = (agent: string, stream = false) => {
	const timelines = createMockTimelines({ now: () => NOW, stream, olderDelay: 0 });
	const updates: TimelineUpdate[] = [];
	const stop = timelines.open(agent, (u) => updates.push(u));
	const reset = updates[0];
	if (reset?.type !== "reset") throw new Error("no reset");
	return { timelines, updates, stop, items: reset.items, reset };
};

afterEach(() => {
	vi.useRealTimers();
});

describe("mock timelines", () => {
	it.each(["agent-usage-popover", "agent-hdr"])("%s holds every kind of item", (agent) => {
		const kinds = new Set(firstPage(agent).items.map((item: TimelineItem) => item.kind));
		// The dispatcher's change cards show only in its own chats.
		const missing = ITEM_KINDS.filter((kind) => !kinds.has(kind));
		expect(missing).toEqual(["dispatcher_action", "dispatcher_step"]);
	});

	it("sends a message with every kind of pill", () => {
		const kinds = new Set(
			firstPage("agent-usage-popover").items.flatMap((item: TimelineItem) =>
				item.kind === "user_message" ? (item.refs ?? []).map((ref) => ref.type) : []
			)
		);
		expect([...kinds].sort()).toEqual(["agent", "attachment", "git_item", "note", "quote"]);
	});

	it("pages ten thousand items of history", async () => {
		const { timelines, updates, reset } = firstPage(LONG_HISTORY);
		expect(reset.items).toHaveLength(PAGE);
		expect(reset.has_older).toBe(true);
		const first = reset.items[0];
		expect(first?.ordinal).toBe(10_000 - PAGE);
		timelines.older(LONG_HISTORY, first?.ordinal ?? 0);
		await vi.waitFor(() => expect(updates).toHaveLength(2));
		const older = updates[1];
		expect(older?.type === "older" && older.items.at(-1)?.ordinal).toBe(10_000 - PAGE - 1);
	});

	it("streams the lead agent's turn while someone listens", () => {
		vi.useFakeTimers();
		const { updates, stop } = firstPage(STREAMING, true);
		vi.advanceTimersByTime(2000);
		const events = updates.filter((u) => u.type === "events");
		expect(events.length).toBeGreaterThan(20);
		const appends = events.some((u) => u.events.some((e) => e.type === "append"));
		expect(appends).toBe(true);
		stop();
		const count = updates.length;
		vi.advanceTimersByTime(2000);
		expect(updates).toHaveLength(count);
	});

	it("marks an approval answered", () => {
		const { timelines, updates } = firstPage("agent-release");
		expect(timelines.decide("att-release", { type: "option", id: "allow" }, "client-1")).toBe(
			true
		);
		const last = updates.at(-1);
		const event = last?.type === "events" ? last.events[0] : undefined;
		const item = event?.type === "upsert" ? event.item : undefined;
		expect(item?.kind === "approval" && item.decided?.by).toBe("client-1");
	});
});
