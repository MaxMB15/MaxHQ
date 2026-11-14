// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";

import type { ActionResult } from "../data/source";
import { agent, device, entities, workspace } from "../test/build";
import { applySnapshot } from "../view/replica";
import { agreeOnMode, agreeToSend, keptConsent, whenModeAllowed } from "./auto-mode";

const memory = () => {
	let kept: string | null = null;
	return keptConsent({
		read: () => kept,
		write: (value) => {
			kept = value;
		},
	});
};

const saying = (answer: boolean) =>
	vi.fn((yes: () => void) => {
		if (answer) yes();
	});

describe("Auto mode's first use", () => {
	it("asks the first time you pick Auto, then not again once you said yes", () => {
		const consent = memory();
		const ask = saying(true);
		const go = vi.fn();
		whenModeAllowed("auto", consent, ask, go);
		whenModeAllowed("auto", consent, ask, go);
		expect(ask).toHaveBeenCalledTimes(1);
		expect(go).toHaveBeenCalledTimes(2);
		expect(consent.confirmed()).toBe(true);
	});

	it("keeps asking while you say no, and doesn't switch", () => {
		const consent = memory();
		const ask = saying(false);
		const go = vi.fn();
		whenModeAllowed("auto", consent, ask, go);
		whenModeAllowed("auto", consent, ask, go);
		expect(ask).toHaveBeenCalledTimes(2);
		expect(go).not.toHaveBeenCalled();
		expect(consent.confirmed()).toBe(false);
	});

	it("never asks about the other modes, and switches right away", () => {
		const ask = vi.fn();
		const go = vi.fn();
		for (const mode of ["ask", "accept_edits", "plan", "full_access"] as const) {
			whenModeAllowed(mode, memory(), ask, go);
		}
		expect(ask).not.toHaveBeenCalled();
		expect(go).toHaveBeenCalledTimes(4);
	});

	it("counts a store it can't read as not confirmed", () => {
		const broken = () => {
			throw new Error("no storage");
		};
		const consent = keptConsent({ read: broken, write: broken });
		expect(consent.confirmed()).toBe(false);
		const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
		expect(() => consent.remember()).not.toThrow();
		warn.mockRestore();
	});
});

const OK: ActionResult = { status: "ok", data: { status: "done" } };
const REFUSED: ActionResult = {
	status: "ok",
	data: { status: "refused", reason: "A phone can't do that" },
};

const autoAgent = agent({
	id: "a1",
	control: "held",
	permission: { requested: "auto", effective: null },
});

const view = applySnapshot({
	seq: 1,
	entities: entities({
		agents: [autoAgent],
		devices: [device({ id: "dev-mac", name: "Mac", os: "macos" })],
		workspaces: [workspace({ id: "w1" })],
	}),
});

const fakeSource = (permission = OK, settings = OK) => ({
	setPermission: vi.fn(async () => permission),
	setDeviceSettings: vi.fn(async () => settings),
});

const place = (source: ReturnType<typeof fakeSource>, agentId: string | null = "a1") => ({
	source,
	view,
	device: "dev-mac",
	agent: agentId,
});

describe("Auto without your pick", () => {
	it("doesn't ask once you confirmed Auto, or for any other mode", async () => {
		const consent = memory();
		consent.remember();
		const ask = vi.fn(async () => "auto" as const);
		const source = fakeSource();
		expect(await agreeOnMode("auto", consent, ask, place(source))).toEqual({
			mode: "auto",
			failure: null,
		});
		expect(await agreeOnMode("ask", memory(), ask, place(source))).toEqual({
			mode: "ask",
			failure: null,
		});
		expect(ask).not.toHaveBeenCalled();
		expect(source.setDeviceSettings).not.toHaveBeenCalled();
	});

	it("keeps Use Auto, so it asks only once", async () => {
		const consent = memory();
		const ask = vi.fn(async () => "auto" as const);
		const source = fakeSource();
		expect(await agreeOnMode("auto", consent, ask, place(source))).toEqual({
			mode: "auto",
			failure: null,
		});
		await agreeOnMode("auto", consent, ask, place(source));
		expect(ask).toHaveBeenCalledTimes(1);
		expect(consent.confirmed()).toBe(true);
		expect(source.setPermission).not.toHaveBeenCalled();
	});

	it("switches the agent and the device's default with Ask first instead", async () => {
		const consent = memory();
		const source = fakeSource();
		const agreed = await agreeOnMode("auto", consent, async () => "ask", place(source));
		expect(agreed).toEqual({ mode: "ask", failure: null });
		expect(source.setPermission).toHaveBeenCalledWith("a1", "ask");
		expect(source.setDeviceSettings).toHaveBeenCalledWith(
			expect.objectContaining({ device: "dev-mac", default_permission: "ask" })
		);
		// Another agent in Auto asks again.
		expect(consent.confirmed()).toBe(false);
	});

	it("only changes the default for an agent that hasn't started", async () => {
		const source = fakeSource();
		const agreed = await agreeOnMode("auto", memory(), async () => "ask", place(source, null));
		expect(agreed).toEqual({ mode: "ask", failure: null });
		expect(source.setPermission).not.toHaveBeenCalled();
		expect(source.setDeviceSettings).toHaveBeenCalledTimes(1);
	});

	it("sends nothing when the agent can't switch", async () => {
		const source = fakeSource(REFUSED);
		const agreed = await agreeOnMode("auto", memory(), async () => "ask", place(source));
		expect(agreed).toEqual({
			mode: null,
			failure: "Couldn't change the permission mode. A phone can't do that.",
		});
	});

	it("goes on in Ask first when only the default can't change, and says so", async () => {
		const source = fakeSource(OK, REFUSED);
		const agreed = await agreeOnMode("auto", memory(), async () => "ask", place(source));
		expect(agreed).toEqual({
			mode: "ask",
			failure: "Couldn't make Ask first the default on Mac. A phone can't do that.",
		});
	});

	it("changes nothing when you dismiss the question", async () => {
		const consent = memory();
		const source = fakeSource();
		const agreed = await agreeOnMode("auto", consent, async () => null, place(source));
		expect(agreed).toEqual({ mode: null, failure: null });
		expect(source.setPermission).not.toHaveBeenCalled();
		expect(source.setDeviceSettings).not.toHaveBeenCalled();
		expect(consent.confirmed()).toBe(false);
	});
});

describe("agreeToSend", () => {
	it("asks about the mode an agent runs in, on its own device", async () => {
		const source = fakeSource();
		const gate = await agreeToSend(autoAgent, memory(), async () => "ask", source, view);
		expect(gate).toEqual({ go: true, failure: null });
		expect(source.setPermission).toHaveBeenCalledWith("a1", "ask");
	});

	it("lets a message go to an agent without a mode, or one running in another", async () => {
		const ask = vi.fn(async () => null);
		const source = fakeSource();
		const observed = { ...autoAgent, permission: null };
		const asking = {
			...autoAgent,
			permission: { requested: "auto", effective: "ask" } as const,
		};
		expect(await agreeToSend(observed, memory(), ask, source, view)).toEqual({
			go: true,
			failure: null,
		});
		expect(await agreeToSend(asking, memory(), ask, source, view)).toEqual({
			go: true,
			failure: null,
		});
		expect(ask).not.toHaveBeenCalled();
	});

	it("holds the message when you dismiss the question", async () => {
		const gate = await agreeToSend(autoAgent, memory(), async () => null, fakeSource(), view);
		expect(gate).toEqual({ go: false, failure: null });
	});
});
