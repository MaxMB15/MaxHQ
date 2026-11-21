// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { FeedCard } from "@maxhq/shared/timeline/output";
import { attention } from "@maxhq/shared/test/build";

import { DETAIL_LINES, moreBelow, promptOfEvent, promptOfItem } from "./prompt";

const options = [
	{ id: "allow", label: "Allow" },
	{ id: "session", label: "Allow for the session" },
	{ id: "deny", label: "Deny" },
];

const approval = (over: Partial<Extract<FeedCard, { kind: "approval" }>> = {}): FeedCard =>
	({
		kind: "approval",
		id: "i1",
		attention: "e1",
		approval_kind: "command",
		title: "Run pnpm test",
		detail: "pnpm test --run",
		cwd: null,
		options,
		decided: null,
		...over,
	}) as unknown as FeedCard;

describe("moreBelow", () => {
	it("tells a detail that runs past what the card shows", () => {
		expect(moreBelow("one line", 1)).toBe(false);
		expect(moreBelow("a long line that wraps", DETAIL_LINES)).toBe(false);
		expect(moreBelow("a long line that wraps", DETAIL_LINES + 1)).toBe(true);
	});

	it("counts line breaks before the layout has measured", () => {
		expect(moreBelow("x\n".repeat(DETAIL_LINES - 1) + "x", 0)).toBe(false);
		expect(moreBelow("x\n".repeat(DETAIL_LINES) + "x", 0)).toBe(true);
	});
});

describe("what a prompt would hide", () => {
	it("is spelled out from the inbox's event", () => {
		const prompt = promptOfEvent(
			attention({ id: "e1", agent: "a1", summary: "Run `rm\u202e…`?" })
		);
		expect(prompt.title).toBe("Run `rm[U+202E]…`?");
		expect(prompt.clipped).toBe(true);
	});

	it("is spelled out from the feed's approval", () => {
		const prompt = promptOfItem(
			approval({
				title: "Run `rm\u202e`?",
				detail: "rm -rf ~\u200b",
				cwd: "/src/\u2066w",
				options: [{ id: "allow", label: "Allow\u202e" }],
			}),
			undefined
		);
		expect(prompt.title).toBe("Run `rm[U+202E]`?");
		expect(prompt.detail).toBe("rm -rf ~[U+200B]");
		expect(prompt.folder).toBe("/src/[U+2066]w");
		expect(prompt.buttons[0]?.option.label).toBe("Allow[U+202E]");
	});
});

describe("promptOfEvent", () => {
	it("lays out a permission's buttons with its yes first and no on the reason field", () => {
		const prompt = promptOfEvent(
			attention({ id: "e1", agent: "a", kind: "permission", options, accepts_text: true })
		);
		expect(prompt.buttons.map((b) => b.tone)).toEqual(["primary", "secondary"]);
		expect(prompt.deny?.id).toBe("deny");
		expect(prompt.acceptsText).toBe(true);
		expect(prompt.permission).toBe(true);
		expect(prompt.clipped).toBe(false);
	});

	it("offers no answer for a permission only its card may answer", () => {
		const prompt = promptOfEvent(
			attention({ id: "e1", agent: "a", kind: "permission", options: [], accepts_text: true })
		);
		expect([prompt.buttons, prompt.deny, prompt.acceptsText]).toEqual([[], null, false]);
	});

	it("says when maxd cut the summary short", () => {
		const event = attention({
			id: "e1",
			agent: "a",
			summary: "Run `cargo test --workspace…`?",
		});
		expect(promptOfEvent(event).clipped).toBe(true);
	});
});

describe("promptOfItem", () => {
	it("shows the full command and, once answered, how", () => {
		expect(promptOfItem(approval(), undefined).detail).toBe("pnpm test --run");
		expect(promptOfItem(approval(), undefined).buttons.at(-1)?.tone).toBe("danger");
		const decided = promptOfItem(
			approval({ decided: { answer: { type: "option", id: "deny" }, by: null, at: 0 } }),
			undefined
		);
		expect(decided.decided).toMatch(/^Deny, in the terminal at /);
	});

	it("says why it asks and puts the no first when the agent wants that", () => {
		const prompt = promptOfItem(
			approval({ deny_first: true, reason: "Deleting ‮a page" }),
			undefined
		);
		expect(prompt.reason).toBe("Deleting [U+202E]a page");
		expect(prompt.buttons.map((b) => [b.option.id, b.tone])).toEqual([
			["deny", "primary"],
			["allow", "secondary"],
			["session", "secondary"],
		]);
		expect(promptOfItem(approval(), undefined).reason).toBeNull();
	});

	it("lists the questions and their answers", () => {
		const item = {
			kind: "question",
			id: "i2",
			attention: "e2",
			questions: [
				{
					id: "q",
					header: "Branch",
					text: "Which branch?",
					options: [],
					multi: false,
					other: true,
					secret: false,
				},
			],
			answers: [{ id: "q", values: ["dev"] }],
		} as unknown as FeedCard;
		const prompt = promptOfItem(item, undefined);
		expect(prompt.title).toBe("Branch");
		expect(prompt.questions).toHaveLength(1);
		expect(prompt.decided).toBe("Answered: dev");
	});

	it("says a question or a request was dismissed, and leaves a closed one as it was", () => {
		const question = {
			kind: "question",
			id: "i4",
			attention: "e4",
			status: "declined",
			questions: [
				{
					id: "q",
					header: null,
					text: "Which branch?",
					options: [],
					multi: false,
					other: true,
					secret: false,
				},
			],
			answers: null,
		} as unknown as FeedCard;
		expect(promptOfItem(question, undefined).decided).toBe("Dismissed");
		expect(promptOfItem(approval({ status: "declined" }), undefined).decided).toBe("Dismissed");
		expect(promptOfItem(approval({ status: "cancelled" }), undefined).decided).toBeNull();
		expect(promptOfItem(approval({ status: "waiting" }), undefined).decided).toBeNull();
	});

	it("titles a card that asks for your consent with what it asks", () => {
		const item = {
			kind: "question",
			id: "i3",
			attention: "e3",
			questions: [
				{
					id: "workflow-env",
					header: "Workflow variables",
					text: "plain text",
					options: [],
					multi: false,
					other: false,
					secret: false,
					consent: {
						title: "Give this agent the workflow's variables?",
						risk: "",
						changes: [],
						rows: [],
						remember: null,
					},
				},
			],
			answers: null,
		} as unknown as FeedCard;
		expect(promptOfItem(item, undefined).title).toBe(
			"Give this agent the workflow's variables?"
		);
	});
});
