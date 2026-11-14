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
	sendLabel,
	sendsNow,
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

describe("sendLabel", () => {
	it("queues while a turn runs and sends when idle", () => {
		expect(sendLabel({ state: "busy" })).toEqual({
			label: "Queue",
			hint: "Sends when the turn ends",
		});
		expect(sendLabel({ state: "idle" })).toEqual({ label: "Send", hint: null });
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

describe("sendsNow", () => {
	const prompted = { state: "needs_input", attention: "att-1" } as const;

	it("offers Send now once you type while a prompt waits", () => {
		expect(sendsNow(prompted, "skip that, run the tests")).toBe(true);
		expect(sendsNow(prompted, " \n ")).toBe(false);
	});

	it("leaves a running turn to Queue and an idle agent to Send", () => {
		expect(sendsNow({ state: "busy" }, "and lint")).toBe(false);
		expect(sendsNow({ state: "idle" }, "hi")).toBe(false);
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
