// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { ANSWER_PILLS_SINCE } from "../bindings";
import { agent, device, entities, link, workspace } from "../test/build";
import { applySnapshot } from "../view/replica";

import { agentPillOf } from "./agent-pill";
import {
	answerBlocked,
	answerPlace,
	draftOf,
	NOTHING_TYPED,
	plainOf,
	shownAnswer,
	takenPiece,
	takesAnswerPills,
	typedFor,
	typedOf,
} from "./answer";
import type { AgentRef, Draft, GitItemRef, NoteRef, Segment } from "./draft";

const RELEASE: AgentRef = {
	type: "agent",
	agent: "a2",
	title: "Release notes",
	vendor: "claude",
	device: "dev-mac",
	excerpt: null,
};
const PLAN: NoteRef = { type: "note", project: "p-a", note: "n1", title: "Plan", copy: null };
const PR: GitItemRef = {
	type: "git_item",
	repo: "owner/repo",
	number: 12,
	kind: "pull_request",
	title: "Fix the build",
	state: "open",
	url: "https://github.com/owner/repo/pull/12",
	head: "fix",
	base: "main",
	body: "",
	checked_at: 0,
};

const text = (value: string): Segment => ({ type: "text", text: value });
const pill = (ref: AgentRef | NoteRef): Segment => ({ type: "pill", pill: ref });

const MAC = link({ id: "mac", name: "Mac", device: "dev-mac", route: { type: "local" } });
const BOX = link({ id: "box", name: "devbox", device: "dev-box" });
const VIEW = applySnapshot({
	seq: 1,
	entities: entities({
		devices: [device({ id: "dev-mac", os: "macos" }), device({ id: "dev-box" })],
		links: [
			{ ...MAC, protocol: ANSWER_PILLS_SINCE },
			{ ...BOX, protocol: ANSWER_PILLS_SINCE - 1 },
		],
		agents: [
			agent({ id: "a1", workspace: "w-box", control: "held" }),
			agent({ id: "a2", title: "Release notes", workspace: "w-mac" }),
		],
		workspaces: [
			workspace({ id: "w-box", device: "dev-box", project: "p-a" }),
			workspace({ id: "w-mac", device: "dev-mac" }),
		],
	}),
});

describe("what you typed", () => {
	it("goes as text with a mark for each pill, and the pills, and comes back", () => {
		const draft: Draft = {
			segments: [text("Like "), pill(RELEASE), text(" and #12")],
			gitItems: [PR],
		};
		const typed = typedOf(draft);
		expect(typed).toEqual({ text: "Like ￼ and #12", refs: [RELEASE, PR] });
		expect(draftOf(typed).segments).toEqual(draft.segments);
		expect(typedOf({ segments: [], gitItems: [] })).toEqual(NOTHING_TYPED);
	});

	it("leaves a file out, as an answer can't carry one", () => {
		const draft: Draft = {
			segments: [text("See "), { type: "pill", pill: { type: "attachment", key: "k1" } }],
			gitItems: [],
		};
		expect(typedOf(draft)).toEqual({ text: "See ", refs: [] });
	});

	it("reads as the words each pill shows for a maxd too old for pills", () => {
		const typed = typedOf({
			segments: [text("Like "), pill(RELEASE), text(", see "), pill(PLAN)],
			gitItems: [],
		});
		expect(plainOf(typed)).toBe("Like [Agent: Release notes], see [Note: Plan]");
		expect(typedFor(typed, true)).toBe(typed);
		expect(typedFor(typed, false)).toEqual({
			text: "Like [Agent: Release notes], see [Note: Plan]",
			refs: [],
		});
		expect(typedFor({ text: "Plain", refs: [] }, false)).toEqual({ text: "Plain", refs: [] });
	});
});

describe("takesAnswerPills", () => {
	it("takes pills where maxd knows them, and not where it's older or unknown", () => {
		expect(takesAnswerPills(VIEW, "dev-mac")).toBe(true);
		expect(takesAnswerPills(VIEW, "dev-box")).toBe(false);
		expect(takesAnswerPills(VIEW, "dev-gone")).toBe(false);
	});
});

describe("answerBlocked", () => {
	const place = { device: "dev-mac", project: "p-a" } as const;

	it("lets an agent on the same device and a note in the same project in", () => {
		expect(answerBlocked(VIEW, [pill(RELEASE), pill(PLAN)], place)).toBeNull();
	});

	it("says why an agent on another device or a note from another project can't go", () => {
		const far = { ...RELEASE, device: "dev-box" };
		expect(answerBlocked(VIEW, [pill(far)], place)).toBe(
			"Release notes runs on devbox, and an answer can't take its excerpt. Send it in a message."
		);
		const other = { ...PLAN, project: "p-b" };
		expect(answerBlocked(VIEW, [pill(other)], place)).toMatch(
			/is in another project, and an answer can't take its copy/
		);
		expect(answerBlocked(VIEW, [pill(PLAN)], { ...place, project: null })).toBe(
			"Notes can't go in here. Send them in a message."
		);
	});

	it("takes an agent from any device where the place names none", () => {
		const far = { ...RELEASE, device: "dev-box" };
		expect(answerBlocked(VIEW, [pill(far)], { device: null, project: null })).toBeNull();
	});
});

describe("answerPlace", () => {
	it("is the device and project of the agent's workspace", () => {
		expect(answerPlace(VIEW, "a1")).toEqual({ device: "dev-box", project: "p-a" });
		expect(answerPlace(VIEW, "a2")).toEqual({ device: "dev-mac", project: null });
		expect(answerPlace(VIEW, "gone")).toBeNull();
	});
});

describe("agentPillOf", () => {
	it("puts the agent on its workspace's device, or the fallback", () => {
		const release = VIEW.tables.agent.get("a2")!;
		expect(agentPillOf(VIEW, release, "dev-box")).toEqual(RELEASE);
		expect(agentPillOf(VIEW, { ...release, workspace: "w-gone" }, "dev-box").device).toBe(
			"dev-box"
		);
	});
});

describe("takenPiece", () => {
	const place = { device: "dev-mac", project: "p-a" } as const;

	it("keeps the pills a field takes as they are", () => {
		const segments = [text("Like "), pill(RELEASE), text(" and "), pill(PLAN)];
		expect(takenPiece(VIEW, segments, place)).toEqual({ segments, why: null });
	});

	it("turns a file and a far agent into their words, and says why", () => {
		const far = { ...RELEASE, device: "dev-box" };
		const taken = takenPiece(
			VIEW,
			[{ type: "pill", pill: { type: "attachment", key: "k1" } }, text(" and "), pill(far)],
			place,
			() => ({ name: "notes.md", kind: "file" })
		);
		expect(taken.segments).toEqual([
			text("[File: notes.md]"),
			text(" and "),
			text("[Agent: Release notes]"),
		]);
		expect(taken.why).toBe("Files can't go in here. Send them in a message.");
	});
});

describe("shownAnswer", () => {
	it("drops the block maxd added for the agent", () => {
		expect(shownAnswer("Like [Agent: Release notes]\n\n<maxhq_context>\n...")).toBe(
			"Like [Agent: Release notes]"
		);
		expect(shownAnswer("Just text")).toBe("Just text");
	});
});
