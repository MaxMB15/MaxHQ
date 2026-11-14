// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { AttachmentRef } from "../bindings";
import { timelineItem } from "../timeline/build";

import { nowMode, type QueuedMessage, queuedLook, waitingMessages } from "./send";

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
