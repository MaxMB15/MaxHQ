// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { AttachmentRef } from "../bindings";
import { timelineItem } from "../timeline/build";

import {
	type BusySend,
	busySendOf,
	nowMode,
	type QueuedMessage,
	queuedLook,
	sendKeys,
	sendModeFor,
	type Turn,
	waitingMessages,
} from "./send";

const file = (id: string, name: string): AttachmentRef => ({
	id,
	name,
	kind: "file",
	bytes: 10,
	mime: null,
	path: `/tmp/${name}`,
	thumbnail: null,
});

const queued = (id: string, ordinal: number, text: string, attachments: AttachmentRef[] = []) =>
	timelineItem(
		{ kind: "user_message", text, attachments, delivery: "queued" },
		{ id, ordinal, status: "waiting" }
	) as QueuedMessage;

describe("waitingMessages", () => {
	it("lists the queued messages oldest first, leaving out the ones taken back", () => {
		const items = [queued("b", 2, "Then lint"), queued("a", 1, "Run the tests")];
		expect(waitingMessages(items, new Set()).map((m) => m.id)).toEqual(["a", "b"]);
		expect(waitingMessages(items, new Set(["a"])).map((m) => m.id)).toEqual(["b"]);
	});
});

describe("queuedLook", () => {
	it("reads the text and counts the files you added", () => {
		const message = queued("a", 1, "Look at this", [file("f1", "meter.png")]);
		expect(queuedLook(message)).toEqual({ text: "Look at this", files: 1 });
	});
});

describe("nowMode", () => {
	it("steers a running turn and sends past a prompt, and waits otherwise", () => {
		expect(nowMode("busy")).toBe("steer");
		expect(nowMode("prompt")).toBe("interrupt");
		expect(nowMode("idle")).toBeNull();
		expect(nowMode("waiting")).toBeNull();
		expect(nowMode("offline")).toBeNull();
	});
});

describe("sendModeFor", () => {
	// [turn, pick, Enter, Command-Enter]
	const cases: [Turn, BusySend, string, string][] = [
		["busy", "queue", "queue", "steer"],
		["busy", "steer", "steer", "queue"],
		["prompt", "queue", "queue", "interrupt"],
		["prompt", "steer", "queue", "interrupt"],
		["idle", "queue", "queue", "queue"],
		["idle", "steer", "queue", "queue"],
		["waiting", "queue", "queue", "queue"],
		["waiting", "steer", "queue", "queue"],
		["offline", "queue", "queue", "queue"],
		["offline", "steer", "queue", "queue"],
	];
	it.each(cases)(
		"sends %s with %s picked: Enter %s, Command-Enter %s",
		(turn, pick, enter, modified) => {
			expect(sendModeFor(turn, pick, false)).toBe(enter);
			expect(sendModeFor(turn, pick, true)).toBe(modified);
		}
	);
});

describe("sendKeys", () => {
	it("gives the second key only while a turn runs or a prompt waits", () => {
		expect(sendKeys("busy", "steer")).toEqual({ enter: "steer", modified: "queue" });
		expect(sendKeys("busy", "queue")).toEqual({ enter: "queue", modified: "steer" });
		expect(sendKeys("prompt", "steer")).toEqual({ enter: "queue", modified: "interrupt" });
		expect(sendKeys("idle", "steer")).toEqual({ enter: "queue", modified: null });
		expect(sendKeys("waiting", "steer")).toEqual({ enter: "queue", modified: null });
	});
});

describe("busySendOf", () => {
	it("reads a stored pick and queues for anything else", () => {
		expect(busySendOf("steer")).toBe("steer");
		expect(busySendOf("queue")).toBe("queue");
		expect(busySendOf("interrupt")).toBe("queue");
		expect(busySendOf(undefined)).toBe("queue");
	});
});
