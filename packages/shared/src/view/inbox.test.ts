// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The same cases as crates/core/src/inbox.rs, so the two stay in step.

import { describe, expect, it } from "vitest";

import type { Activity, Agent, Inbox } from "../bindings";
import { agent as build } from "../test/build";

import {
	apply,
	autoSettled,
	autoSettles,
	DEFAULT_INBOX,
	inboxOf,
	isSnoozed,
	onMessage,
	settle,
	wakesEarly,
} from "./inbox";

const DAY = 24 * 60 * 60 * 1000;
const NOW = 100 * DAY;
const at = (daysAgo: number) => NOW - daysAgo * DAY;

const agent = (activity: Activity, idleDays: number, inbox: Inbox = DEFAULT_INBOX): Agent =>
	build({
		id: "a1",
		activity,
		started_at: at(30),
		last_activity_at: at(idleDays),
		inbox,
	});

const pinned = (key: string): Inbox => ({ ...DEFAULT_INBOX, pin: { at: at(3), key } });

describe("applying a change", () => {
	it("settling unpins and ends the snooze", () => {
		const inbox = { ...pinned("m"), snoozed_until: at(0) + 60_000, snoozed_at: at(1) };
		const next = apply(inbox, { change: "settle" }, NOW);
		expect(next.settled).toEqual({ at: NOW, by: "you" });
		expect(next.pin).toBeNull();
		expect(next.snoozed_until).toBeNull();
		// Settling isn't waking.
		expect(next.woke_at).toBeNull();
	});

	it("settling again keeps the first settle", () => {
		const first = apply(DEFAULT_INBOX, { change: "settle" }, at(2));
		const again = apply(first, { change: "settle" }, NOW);
		expect(again.settled?.at).toBe(at(2));
	});

	it("unsettling keeps auto-settle away until a message", () => {
		const settled = apply(DEFAULT_INBOX, { change: "settle" }, at(1));
		const back = apply(settled, { change: "unsettle" }, NOW);
		expect(back.settled).toBeNull();
		expect(back.kept_active).toBe(true);
		expect(onMessage(back).kept_active).toBe(false);
	});

	it("pinning brings a settled or snoozed agent back", () => {
		const snoozed: Inbox = {
			...DEFAULT_INBOX,
			settled: { at: at(1), by: "auto" },
			snoozed_until: at(0) + 60_000,
			snoozed_at: at(1),
		};
		const next = apply(snoozed, { change: "pin", key: "m" }, NOW);
		expect(next.settled).toBeNull();
		expect(next.kept_active).toBe(true);
		expect(next.snoozed_until).toBeNull();
		expect(next.woke_at).toBe(NOW);
		expect(next.pin).toEqual({ at: NOW, key: "m" });
	});

	it("pinning a pinned agent moves it and keeps when it was pinned", () => {
		const next = apply(pinned("m"), { change: "pin", key: "c" }, NOW);
		expect(next.pin).toEqual({ at: at(3), key: "c" });
	});

	it("waking leaves a marker until dismissed", () => {
		const snoozed = apply(DEFAULT_INBOX, { change: "snooze", until: at(0) + 3_600_000 }, NOW);
		expect(isSnoozed(snoozed, NOW)).toBe(true);
		const woke = apply(snoozed, { change: "wake" }, NOW);
		expect(isSnoozed(woke, NOW)).toBe(false);
		expect(woke.woke_at).toBe(NOW);
		const dismissed = apply(woke, { change: "dismiss_woke" }, NOW);
		expect(dismissed.woke_at).toBeNull();
	});

	it("waking an agent that isn't snoozed leaves no marker", () => {
		expect(apply(DEFAULT_INBOX, { change: "wake" }, NOW)).toEqual(DEFAULT_INBOX);
	});

	it("a snooze ends on time without maxd", () => {
		const snoozed = { ...DEFAULT_INBOX, snoozed_until: at(1), snoozed_at: at(2) };
		expect(isSnoozed(snoozed, NOW)).toBe(false);
	});

	it("a message ends a snooze without a marker", () => {
		const inbox = { ...DEFAULT_INBOX, snoozed_until: at(0) + 60_000, snoozed_at: at(1) };
		expect(onMessage(inbox)).toEqual(DEFAULT_INBOX);
	});

	it("restoring drops a snooze that has ended", () => {
		const before = { ...pinned("m"), snoozed_until: at(1), snoozed_at: at(2) };
		const settled = apply(before, { change: "settle" }, NOW);
		const restored = apply(settled, { change: "restore", inbox: before }, NOW);
		expect(restored).toEqual(pinned("m"));
	});

	it("turning auto-settle off and on again", () => {
		const off = apply(DEFAULT_INBOX, { change: "auto_settle", on: false }, NOW);
		expect(off.auto_settle_off).toBe(true);
		expect(apply(off, { change: "auto_settle", on: true }, NOW).auto_settle_off).toBe(false);
	});

	it("turning continuing after the limit on and off", () => {
		const on = apply(DEFAULT_INBOX, { change: "continue_at_limit", on: true }, NOW);
		expect(on.continue_at_limit).toBe(true);
		expect(apply(on, { change: "continue_at_limit", on: false }, NOW).continue_at_limit).toBe(
			false
		);
	});

	it("leaves the inbox it was given alone", () => {
		const inbox = pinned("m");
		apply(inbox, { change: "settle" }, NOW);
		expect(inbox).toEqual(pinned("m"));
	});
});

describe("an agent from a maxd before inboxes", () => {
	it("reads as active", () => {
		const old = agent({ state: "idle" }, 1);
		delete old.inbox;
		expect(inboxOf(old)).toEqual(DEFAULT_INBOX);
	});
});

describe("waking early", () => {
	it("happens when it needs you or fails, and not when it finishes", () => {
		expect(wakesEarly({ state: "needs_input", attention: "att-1" })).toBe(true);
		expect(wakesEarly({ state: "error", reason: "exited" })).toBe(true);
		expect(wakesEarly({ state: "paused", reason: "resumes_made_no_progress" })).toBe(true);
		expect(wakesEarly({ state: "done_unread" })).toBe(false);
		expect(wakesEarly({ state: "idle" })).toBe(false);
	});
});

describe("auto-settle", () => {
	it("takes an agent idle for the set days", () => {
		expect(autoSettles(agent({ state: "idle" }, 5), 5, false, NOW)).toBe(true);
		expect(autoSettles(agent({ state: "idle" }, 4), 5, false, NOW)).toBe(false);
		const stopped: Activity = { state: "paused", reason: "by_you" };
		expect(autoSettles(agent(stopped, 6), 5, false, NOW)).toBe(true);
	});

	it("keeps what should stay in view", () => {
		const idle = agent({ state: "idle" }, 9);
		expect(autoSettles(idle, 0, false, NOW), "off").toBe(false);
		expect(autoSettles(idle, 5, true, NOW), "unsaved").toBe(false);
		expect(autoSettles(idle, 5, null, NOW), "unknown").toBe(false);
		const busy: Activity[] = [
			{ state: "done_unread" },
			{ state: "busy" },
			{ state: "starting" },
			{ state: "needs_input", attention: "att-1" },
			{ state: "waiting_for_limit_reset", resets_at: null },
			{ state: "error", reason: "exited" },
			{ state: "paused", reason: "resumes_made_no_progress" },
		];
		for (const activity of busy) {
			expect(autoSettles(agent(activity, 9), 5, false, NOW), activity.state).toBe(false);
		}
		const kept = (inbox: Inbox) =>
			autoSettles(agent({ state: "idle" }, 9, inbox), 5, false, NOW);
		expect(kept(pinned("m")), "pinned").toBe(false);
		expect(kept({ ...DEFAULT_INBOX, kept_active: true }), "unsettled by hand").toBe(false);
		expect(kept({ ...DEFAULT_INBOX, auto_settle_off: true }), "turned off").toBe(false);
		expect(kept({ ...DEFAULT_INBOX, snoozed_until: at(0) + 60_000 }), "snoozed").toBe(false);
		const sub = { ...idle, parent: "lead" };
		expect(autoSettles(sub, 5, false, NOW), "sub-agent").toBe(false);
	});

	it("dates the settle to the last activity", () => {
		const idle = agent({ state: "idle" }, 7);
		expect(settle(DEFAULT_INBOX, autoSettled(idle)).settled).toEqual({ at: at(7), by: "auto" });
	});
});
