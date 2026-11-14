// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { DISMISS_SINCE, type ItemStatus } from "../bindings";
import { device, entities, link } from "../test/build";
import { timelineItem } from "../timeline/build";
import type { FeedCard } from "../timeline/output";
import { applySnapshot } from "../view/replica";

import { dismissBlocked, dismissSent, SKIP_WORDS, takesDismiss, wasDismissed } from "./dismiss";

const VIEW = applySnapshot({
	seq: 1,
	entities: entities({
		devices: [device({ id: "dev-mac", os: "macos" }), device({ id: "dev-box" })],
		links: [
			{ ...link({ id: "mac", name: "Mac", device: "dev-mac" }), protocol: DISMISS_SINCE },
			{
				...link({ id: "box", name: "devbox", device: "dev-box" }),
				protocol: DISMISS_SINCE - 1,
			},
		],
	}),
});

const question = (status: ItemStatus, answered: boolean) =>
	timelineItem(
		{
			kind: "question",
			attention: "att-1",
			questions: [
				{
					id: "q1",
					header: null,
					text: "Which?",
					options: [],
					multi: false,
					other: true,
					secret: false,
				},
			],
			answers: answered ? [{ id: "q1", values: ["This one"] }] : null,
		},
		{ id: "q", status }
	) as FeedCard;

const approval = (status: ItemStatus, decided: boolean) =>
	timelineItem(
		{
			kind: "approval",
			attention: "att-2",
			approval_kind: "command",
			title: "Run `ls`",
			detail: null,
			cwd: null,
			options: [{ id: "deny", label: "Deny" }],
			decided: decided ? { answer: { type: "option", id: "deny" }, by: null, at: 0 } : null,
		},
		{ id: "a", status }
	) as FeedCard;

describe("takesDismiss", () => {
	it("dismisses where maxd knows how, and not where it's older or unknown", () => {
		expect(takesDismiss(VIEW, "dev-mac")).toBe(true);
		expect(takesDismiss(VIEW, "dev-box")).toBe(false);
		expect(takesDismiss(VIEW, "dev-gone")).toBe(false);
	});
});

describe("dismissBlocked", () => {
	it("says which device needs a newer maxd", () => {
		expect(dismissBlocked(VIEW, "dev-mac")).toBeNull();
		expect(dismissBlocked(VIEW, "dev-box")).toBe("Dismiss needs a newer maxd on devbox.");
		expect(dismissBlocked(VIEW, null)).toBeNull();
	});
});

describe("wasDismissed", () => {
	it("reads a card turned down with nothing answered or decided as dismissed", () => {
		expect(wasDismissed(question("declined", false))).toBe(true);
		expect(wasDismissed(approval("declined", false))).toBe(true);
	});

	it("leaves answered, decided, open and closed cards alone", () => {
		expect(wasDismissed(question("completed", true))).toBe(false);
		expect(wasDismissed(question("waiting", false))).toBe(false);
		expect(wasDismissed(question("cancelled", false))).toBe(false);
		expect(wasDismissed(approval("declined", true))).toBe(false);
		expect(wasDismissed(approval("cancelled", false))).toBe(false);
	});
});

describe("dismissSent", () => {
	it("says the card was dismissed and whose turn stops, not that an answer went", () => {
		expect(dismissSent("claude")).toBe("Dismissed. Stopping Claude's turn.");
		expect(dismissSent("codex")).toBe("Dismissed. Stopping Codex's turn.");
	});
});

describe("SKIP_WORDS", () => {
	it("says what Dismiss and Send now do", () => {
		expect(SKIP_WORDS.dismiss).toBe("Dismiss");
		expect(SKIP_WORDS.dismissTip).toBe("Skip these questions and stop the turn");
		expect(SKIP_WORDS.sendNow).toBe("Send now");
		expect(SKIP_WORDS.sendNowTip).toBe("Skips what's waiting and sends this now");
	});
});
