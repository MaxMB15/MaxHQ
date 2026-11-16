// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";

import type { AgentId, AttachmentRef, DeviceId } from "@maxhq/shared/bindings";
import { createStaging, type PickedFile } from "@maxhq/shared/composer/staging";
import type { ActionResult, DataSource } from "@maxhq/shared/data/source";
import { OBJECT_MARK } from "@maxhq/shared/pills/draft";
import { createGitPills } from "@maxhq/shared/pills/git-pills";
import {
	agreeToSend,
	type AutoChoice,
	type AutoConsent,
	keptConsent,
} from "@maxhq/shared/settings/auto-mode";
import { agent, device, entities, workspace } from "@maxhq/shared/test/build";
import { applySnapshot } from "@maxhq/shared/view/replica";

import type { PhoneDraft } from "./draft";
import { sendDraft, type SendSetup } from "./send";

const DONE: ActionResult = { status: "ok", data: { status: "done" } };
const AGENT = "a1" as AgentId;
const MAC = "dev-mac" as DeviceId;

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

const photo: AttachmentRef = {
	id: "att-1",
	name: "photo.jpg",
	kind: "image",
	bytes: 3,
	mime: "image/jpeg",
	path: "/staged/photo.jpg",
	thumbnail: null,
};

const picked: PickedFile = {
	name: "photo.jpg",
	mime: "image/jpeg",
	size: 3,
	preview: null,
	read: async () => new Uint8Array([1, 2, 3]),
};

const phone = (
	results: { settings?: ActionResult; pasted?: ActionResult; send?: ActionResult } = {}
) => ({
	setPermission: vi.fn(async () => DONE),
	setDeviceSettings: vi.fn(async () => results.settings ?? DONE),
	sendInput: vi.fn(async () => results.send ?? DONE),
	stagePasted: vi.fn(
		async () =>
			results.pasted ??
			({ status: "ok", data: { status: "staged", attachment: photo } } as ActionResult)
	),
	stageFound: vi.fn(async () => []),
	findGitItems: vi.fn(async () => ({ items: [] })),
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

const setupOf = (
	source: ReturnType<typeof phone>,
	consent: AutoConsent,
	choice: AutoChoice | null
): SendSetup => ({
	source: source as unknown as DataSource,
	view: () => view,
	agent: AGENT,
	device: MAC,
	ownProject: null,
	staging: createStaging(source as never, MAC),
	gitPills: createGitPills(async () => null),
	agree: () => agreeToSend(inAuto, consent, async () => choice, source as never, view),
});

const words: PhoneDraft = { segments: [{ type: "text", text: "Fix the meter" }], loose: [] };

describe("sendDraft", () => {
	it("sends in Auto once you say Use Auto, and asks once per install", async () => {
		const source = phone();
		const consent = memory();
		const sent = await sendDraft(setupOf(source, consent, "auto"), words, "queue");
		expect(sent).toEqual({ sent: true, failure: null });
		expect(consent.confirmed()).toBe(true);
		expect(source.setPermission).not.toHaveBeenCalled();
		expect(source.sendInput).toHaveBeenCalledWith(
			AGENT,
			{ text: "Fix the meter", attachments: [] },
			"queue"
		);
	});

	it("switches the agent and the Mac's default with Ask first instead, then sends", async () => {
		const source = phone();
		const sent = await sendDraft(setupOf(source, memory(), "ask"), words, "queue");
		expect(sent).toEqual({ sent: true, failure: null });
		expect(source.setPermission).toHaveBeenCalledWith("a1", "ask");
		expect(source.setDeviceSettings).toHaveBeenCalledWith(
			expect.objectContaining({ device: "dev-mac", default_permission: "ask" })
		);
		expect(source.sendInput).toHaveBeenCalledTimes(1);
	});

	it("still sends when the Mac won't take a default from the phone, and says so", async () => {
		const source = phone({
			settings: {
				status: "ok",
				data: { status: "refused", reason: "A phone can't do that" },
			},
		});
		expect(await sendDraft(setupOf(source, memory(), "ask"), words, "queue")).toEqual({
			sent: true,
			failure: "Couldn't make Ask first the default on Mac. A phone can't do that.",
		});
	});

	it("keeps the message when you dismiss the question", async () => {
		const source = phone();
		expect(await sendDraft(setupOf(source, memory(), null), words, "queue")).toEqual({
			sent: false,
			failure: null,
		});
		expect(source.sendInput).not.toHaveBeenCalled();
	});

	it("waits for a photo in the strip and sends it after the words", async () => {
		const source = phone();
		const consent = memory();
		consent.remember();
		const setup = setupOf(source, consent, "auto");
		const waits: boolean[] = [];
		const result = setup.staging.getState().addPicked(picked);
		if (!("key" in result)) throw new Error("refused");
		const draft: PhoneDraft = { segments: words.segments, loose: [result.key] };
		const sent = await sendDraft(
			{ ...setup, onWait: (waiting) => waits.push(waiting) },
			draft,
			"steer"
		);
		expect(sent).toEqual({ sent: true, failure: null });
		expect(waits).toEqual([true, false]);
		expect(source.sendInput).toHaveBeenCalledWith(
			AGENT,
			{
				text: `Fix the meter ${OBJECT_MARK}`,
				attachments: ["att-1"],
				refs: [{ type: "attachment", id: "att-1" }],
			},
			"steer"
		);
	});

	it("keeps the message and says why when a photo couldn't get ready", async () => {
		const source = phone({
			pasted: { status: "error", error: { kind: "failed", message: "Disk full" } },
		});
		const consent = memory();
		consent.remember();
		const setup = setupOf(source, consent, "auto");
		const result = setup.staging.getState().addPicked(picked);
		if (!("key" in result)) throw new Error("refused");
		const sent = await sendDraft(setup, { segments: [], loose: [result.key] }, "queue");
		expect(sent.sent).toBe(false);
		expect(sent.failure).toContain("photo.jpg");
		expect(source.sendInput).not.toHaveBeenCalled();
	});

	it("shows why the agent didn't take it", async () => {
		const source = phone({
			send: { status: "error", error: { kind: "failed", message: "Gone" } },
		});
		const consent = memory();
		consent.remember();
		const sent = await sendDraft(setupOf(source, consent, "auto"), words, "queue");
		expect(sent).toEqual({ sent: false, failure: expect.stringContaining("Gone") });
	});
});
