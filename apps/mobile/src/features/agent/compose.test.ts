// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";

import { clearKept, keep, keptValue } from "@maxhq/shared/agent-view/kept";
import type { ActionResult } from "@maxhq/shared/data/source";
import {
	agreeToSend,
	type AutoChoice,
	type AutoConsent,
	keptConsent,
} from "@maxhq/shared/settings/auto-mode";
import { agent, device, entities, workspace } from "@maxhq/shared/test/build";
import { applySnapshot } from "@maxhq/shared/view/replica";

import {
	canSend,
	continueTone,
	draftKey,
	hasComposer,
	inputOf,
	sendAgreed,
	otherSend,
	sendLabel,
	sendMode,
	stopsTurn,
} from "./compose";

const DONE: ActionResult = { status: "ok", data: { status: "done" } };

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

describe("inputOf", () => {
	it("sends the trimmed text and nothing for blank text", () => {
		expect(inputOf("  fix it \n")).toEqual({ text: "fix it", attachments: [] });
		expect(inputOf(" \n ")).toBeNull();
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
		expect(stopsTurn({ state: "busy" }, "")).toBe(true);
		expect(stopsTurn({ state: "busy" }, " \n ")).toBe(true);
	});

	it("queues once you type, and has nothing to stop when idle", () => {
		expect(stopsTurn({ state: "busy" }, "and lint")).toBe(false);
		expect(stopsTurn({ state: "idle" }, "")).toBe(false);
	});
});

describe("otherSend", () => {
	it("offers Send now once you type while a prompt waits, whatever the pick", () => {
		for (const pick of ["queue", "steer"] as const) {
			expect(otherSend(prompted, pick, "skip that, run the tests")).toEqual({
				mode: "interrupt",
				label: "Send now",
				hint: "Skips what's waiting and sends this now",
			});
			expect(otherSend(prompted, pick, " \n ")).toBeNull();
		}
	});

	it("offers the action you didn't pick once you type while a turn runs", () => {
		expect(otherSend({ state: "busy" }, "queue", "and lint")).toMatchObject({
			mode: "steer",
			label: "Steer",
		});
		expect(otherSend({ state: "busy" }, "steer", "and lint")).toMatchObject({
			mode: "queue",
			label: "Queue",
		});
		expect(otherSend({ state: "busy" }, "steer", "")).toBeNull();
	});

	it("offers nothing more to an idle agent or one waiting for its limit", () => {
		expect(otherSend({ state: "idle" }, "steer", "hi")).toBeNull();
		expect(
			otherSend({ state: "waiting_for_limit_reset", resets_at: null }, "steer", "hi")
		).toBeNull();
	});
});

describe("sendAgreed", () => {
	// A held agent on the Mac in Auto, which its default put it in.
	const inAuto = agent({
		id: "a1",
		control: "held",
		permission: { requested: "auto", effective: null },
	});
	const view = applySnapshot({
		seq: 1,
		entities: entities({
			agents: [inAuto],
			devices: [device({ id: "dev-mac", name: "Mac", os: "macos" })],
			workspaces: [workspace({ id: "w1" })],
		}),
	});
	const memory = () => {
		let kept: string | null = null;
		return keptConsent({
			read: () => kept,
			write: (value) => {
				kept = value;
			},
		});
	};
	const phone = (settings: ActionResult = DONE) => ({
		setPermission: vi.fn(async () => DONE),
		setDeviceSettings: vi.fn(async () => settings),
		sendInput: vi.fn(async () => DONE),
	});
	const press = (
		source: ReturnType<typeof phone>,
		consent: AutoConsent,
		choice: AutoChoice | null
	) =>
		sendAgreed(
			() => agreeToSend(inAuto, consent, async () => choice, source, view),
			() => source.sendInput()
		);

	it("sends in Auto once you say Use Auto, and asks once per install", async () => {
		const source = phone();
		const consent = memory();
		expect(await press(source, consent, "auto")).toEqual({ sent: true, failure: null });
		expect(consent.confirmed()).toBe(true);
		expect(source.setPermission).not.toHaveBeenCalled();
		expect(source.sendInput).toHaveBeenCalledTimes(1);
	});

	it("switches the agent and the Mac's default with Ask first instead, then sends", async () => {
		const source = phone();
		expect(await press(source, memory(), "ask")).toEqual({ sent: true, failure: null });
		expect(source.setPermission).toHaveBeenCalledWith("a1", "ask");
		expect(source.setDeviceSettings).toHaveBeenCalledWith(
			expect.objectContaining({ device: "dev-mac", default_permission: "ask" })
		);
		expect(source.sendInput).toHaveBeenCalledTimes(1);
	});

	it("still sends when the Mac won't take a default from the phone, and says so", async () => {
		const source = phone({
			status: "ok",
			data: { status: "refused", reason: "A phone can't do that" },
		});
		expect(await press(source, memory(), "ask")).toEqual({
			sent: true,
			failure: "Couldn't make Ask first the default on Mac. A phone can't do that.",
		});
	});

	it("keeps the message when you dismiss the question", async () => {
		const source = phone();
		expect(await press(source, memory(), null)).toEqual({ sent: false, failure: null });
		expect(source.sendInput).not.toHaveBeenCalled();
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
