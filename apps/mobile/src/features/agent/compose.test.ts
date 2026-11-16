// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { clearKept, keep, keptValue } from "@maxhq/shared/agent-view/kept";
import { agent } from "@maxhq/shared/test/build";

import {
	canSend,
	continueTone,
	draftKey,
	hasComposer,
	otherSend,
	sendLabel,
	sendMode,
	stopsTurn,
} from "./compose";

describe("hasComposer", () => {
	it("gives a session started in a terminal no compose box", () => {
		expect(hasComposer(agent({ id: "a1", control: "observed" }))).toBe(false);
		expect(hasComposer(agent({ id: "a1", control: "held" }))).toBe(true);
	});
});

describe("continueTone", () => {
	it("draws Continue in MaxHQ quiet until the terminal lets go", () => {
		expect(continueTone(false)).toBe("secondary");
		expect(continueTone(true)).toBe("primary");
	});
});

describe("canSend", () => {
	it("refuses only an agent out of reach", () => {
		expect(canSend({ state: "busy" })).toBe(true);
		expect(canSend({ state: "not_accessible" })).toBe(false);
	});
});

const prompted = { state: "needs_input", attention: "att-1" } as const;

describe("the send button", () => {
	it("queues while a turn runs and sends when idle, by default", () => {
		expect(sendLabel({ state: "busy" }, "queue")).toEqual({
			label: "Queue",
			hint: "Sends when the turn ends",
		});
		expect(sendMode({ state: "busy" }, "queue")).toBe("queue");
		expect(sendLabel({ state: "idle" }, "queue")).toEqual({ label: "Send", hint: null });
		expect(sendMode({ state: "idle" }, "queue")).toBe("queue");
	});

	it("steers a running turn with Steer picked", () => {
		expect(sendLabel({ state: "busy" }, "steer")).toEqual({
			label: "Steer",
			hint: "The agent reads it between steps",
		});
		expect(sendMode({ state: "busy" }, "steer")).toBe("steer");
	});

	it("queues while a prompt waits and sends when idle, whatever the pick", () => {
		expect(sendLabel(prompted, "steer").label).toBe("Queue");
		expect(sendMode(prompted, "steer")).toBe("queue");
		expect(sendLabel({ state: "idle" }, "steer")).toEqual({ label: "Send", hint: null });
		expect(sendMode({ state: "idle" }, "steer")).toBe("queue");
	});
});

describe("stopsTurn", () => {
	it("stops a running turn while the box is empty", () => {
		expect(stopsTurn({ state: "busy" }, false)).toBe(true);
	});

	it("queues once you type, and has nothing to stop when idle", () => {
		expect(stopsTurn({ state: "busy" }, true)).toBe(false);
		expect(stopsTurn({ state: "idle" }, false)).toBe(false);
	});
});

describe("otherSend", () => {
	it("offers Send now once you type while a prompt waits, whatever the pick", () => {
		for (const pick of ["queue", "steer"] as const) {
			expect(otherSend(prompted, pick, true)).toEqual({
				mode: "interrupt",
				label: "Send now",
				hint: "Skips what's waiting and sends this now",
			});
			expect(otherSend(prompted, pick, false)).toBeNull();
		}
	});

	it("offers the action you didn't pick once you type while a turn runs", () => {
		expect(otherSend({ state: "busy" }, "queue", true)).toMatchObject({
			mode: "steer",
			label: "Steer",
		});
		expect(otherSend({ state: "busy" }, "steer", true)).toMatchObject({
			mode: "queue",
			label: "Queue",
		});
		expect(otherSend({ state: "busy" }, "steer", false)).toBeNull();
	});

	it("offers nothing more to an idle agent or one waiting for its limit", () => {
		expect(otherSend({ state: "idle" }, "steer", true)).toBeNull();
		expect(
			otherSend({ state: "waiting_for_limit_reset", resets_at: null }, "steer", true)
		).toBeNull();
	});
});

describe("draftKey", () => {
	it("keeps a draft for each agent", () => {
		clearKept();
		keep(draftKey("a1"), "Fix the meter");
		expect(keptValue(draftKey("a2"), "")).toBe("");
		expect(keptValue(draftKey("a1"), "")).toBe("Fix the meter");
	});
});
