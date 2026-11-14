// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";

import type { TimelineItem } from "../bindings";
import type { ActionResult } from "../data/source";
import { timelineItem } from "../timeline/build";
import {
	attempt,
	limitWaitBody,
	limitWaitEnded,
	limitWaitTitle,
	stopResume,
	takeBack,
} from "./limit-wait";

const OK: ActionResult = { status: "ok", data: { status: "done" } };
const REFUSED: ActionResult = { status: "ok", data: { status: "refused", reason: "it has gone" } };

const message = (id: string): TimelineItem =>
	timelineItem(
		{ kind: "user_message", text: "Go on", attachments: [], delivery: "queued" },
		{ id }
	);

describe("limitWaitEnded", () => {
	it("says how long it waited when it knows", () => {
		expect(limitWaitEnded(65_000)).toBe("Waited 1 min 5 s for the usage limit to reset");
		expect(limitWaitEnded(null)).toBe("Waited for the usage limit to reset");
	});
});

describe("limitWaitTitle", () => {
	it("names the time only when it knows the reset", () => {
		expect(limitWaitTitle(null)).toBe("Waiting for usage limit reset");
		expect(limitWaitTitle(1_793_361_600_000)).toMatch(
			/^Waiting for usage limit reset, resumes /
		);
	});
});

describe("limitWaitBody", () => {
	it("tells you to send a message when nothing would resume it", () => {
		expect(limitWaitBody(undefined, false, null, 0)).toBe(
			"The account is at its limit. Send a message once it resets to carry on."
		);
	});

	it("counts down to the reset when it resumes", () => {
		expect(limitWaitBody(undefined, true, 65_000, 0)).toBe(
			"The account is at its limit. This agent continues on its own in 1 min 5 s, and nothing is lost meanwhile."
		);
	});

	it("stops the count at nothing past the reset", () => {
		expect(limitWaitBody(undefined, true, 1000, 5000)).toContain("on its own in 0 s,");
	});

	it("still says it resumes when the reset isn't known", () => {
		expect(limitWaitBody(undefined, true, null, 0)).toBe(
			"The account is at its limit. This agent continues on its own once it resets, and nothing is lost meanwhile."
		);
	});
});

describe("attempt", () => {
	it("gives null when the action went through", async () => {
		await expect(attempt(() => Promise.resolve(OK))).resolves.toBeNull();
	});

	it("gives the reason when it was refused or threw", async () => {
		await expect(attempt(() => Promise.resolve(REFUSED))).resolves.toBe("It has gone.");
		await expect(attempt(() => Promise.reject(new Error("No link")))).resolves.toBe("No link");
		await expect(attempt(() => Promise.reject(new Error("")))).resolves.toBe(
			"It didn't go through."
		);
	});
});

describe("takeBack", () => {
	it("takes back each waiting message in order", async () => {
		const cancel = vi.fn(() => Promise.resolve(OK));
		await expect(takeBack(cancel, [message("a"), message("b")])).resolves.toBeNull();
		expect(cancel.mock.calls).toEqual([["a"], ["b"]]);
	});

	it("stops at the first one that stays and says why", async () => {
		const cancel = vi.fn(() => Promise.resolve(REFUSED));
		await expect(takeBack(cancel, [message("a"), message("b")])).resolves.toBe("It has gone.");
		expect(cancel).toHaveBeenCalledTimes(1);
	});
});

describe("stopResume", () => {
	it("turns the switch off before taking back the messages", async () => {
		const calls: string[] = [];
		const source = {
			setInbox: vi.fn(async () => {
				calls.push("switch");
				return [OK];
			}),
			cancelQueued: vi.fn(async (_agent: string, item: string) => {
				calls.push(item);
				return OK;
			}),
		};
		expect(await stopResume(source, "a", [message("m1")], true)).toBeNull();
		expect(source.setInbox).toHaveBeenCalledWith(["a"], {
			change: "continue_at_limit",
			on: false,
		});
		expect(calls).toEqual(["switch", "m1"]);
	});

	it("leaves the switch alone when it doesn't carry on", async () => {
		const source = { setInbox: vi.fn(async () => [OK]), cancelQueued: vi.fn(async () => OK) };
		expect(await stopResume(source, "a", [message("m1")], false)).toBeNull();
		expect(source.setInbox).not.toHaveBeenCalled();
	});

	it("says why the switch stayed on and keeps the messages", async () => {
		const source = {
			setInbox: vi.fn(async () => [REFUSED]),
			cancelQueued: vi.fn(async () => OK),
		};
		expect(await stopResume(source, "a", [message("m1")], true)).toBe(
			"Couldn't turn off continuing after the limit. It has gone."
		);
		expect(source.cancelQueued).not.toHaveBeenCalled();
	});
});
