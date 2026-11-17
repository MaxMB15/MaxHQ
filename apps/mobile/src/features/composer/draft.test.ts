// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import type { AttachmentRef, InlineRef } from "@maxhq/shared/bindings";
import type { QueuedMessage } from "@maxhq/shared/composer/send";
import type { StagedFile } from "@maxhq/shared/composer/staging";
import { type GitItemRef, OBJECT_MARK, type Segment } from "@maxhq/shared/pills/draft";
import { describe, expect, it } from "vitest";

import {
	draftKeys,
	draftOfFragment,
	EMPTY_DRAFT,
	filled,
	fragmentOf,
	messageInput,
	queuedDraft,
	queuedFiles,
	queuedInput,
	splitLoose,
	unreadyFiles,
	wholeOf,
	withRestored,
} from "./draft";

const M = OBJECT_MARK;

const text = (value: string): Segment => ({ type: "text", text: value });
const file = (key: string): Segment => ({ type: "pill", pill: { type: "attachment", key } });

const ref = (id: string, name = `${id}.png`): AttachmentRef => ({
	id,
	name,
	kind: "image",
	bytes: 10,
	mime: "image/png",
	path: `/staged/${name}`,
	thumbnail: null,
});

const staged = (key: string, over: Partial<StagedFile> = {}): StagedFile => ({
	key,
	name: `${key}.png`,
	kind: "image",
	bytes: 10,
	preview: null,
	state: "ready",
	ref: ref(`att-${key}`, `${key}.png`),
	error: null,
	found: null,
	upload: null,
	...over,
});

const pr: GitItemRef = {
	type: "git_item",
	repo: "owner/repo",
	number: 12,
	kind: "pull_request",
	title: "Cache usage",
	state: "open",
	url: "https://github.com/owner/repo/pull/12",
	head: "usage-cache",
	base: "main",
	body: "",
	checked_at: 1_793_361_600_000,
};

const queued = (body: string, attachments: AttachmentRef[], refs?: InlineRef[]): QueuedMessage =>
	({
		id: "item-1",
		agent: "agent-1",
		kind: "user_message",
		text: body,
		attachments,
		delivery: "queued",
		...(refs ? { refs } : {}),
	}) as unknown as QueuedMessage;

describe("wholeOf and splitLoose", () => {
	it("puts the strip's files after the text, each after a space", () => {
		expect(wholeOf({ segments: [text("look at these \n")], loose: ["a", "b"] })).toEqual([
			text("look at these "),
			file("a"),
			text(" "),
			file("b"),
		]);
	});

	it("keeps a draft with no files in the strip as it is", () => {
		expect(wholeOf({ segments: [text("hi "), file("a")], loose: [] })).toEqual([
			text("hi "),
			file("a"),
		]);
		expect(wholeOf({ segments: [], loose: ["a"] })).toEqual([file("a")]);
	});

	it("gives the files at the end back to the strip", () => {
		const draft = { segments: [text("see "), file("a"), text(" and")], loose: ["b", "c"] };
		expect(splitLoose(wholeOf(draft))).toEqual(draft);
	});

	it("leaves a file inside the text where it is", () => {
		expect(splitLoose([text("see "), file("a"), text(" there")])).toEqual({
			segments: [text("see "), file("a"), text(" there")],
			loose: [],
		});
	});

	it("takes a message that is only files", () => {
		expect(splitLoose([file("a"), text(" "), file("b")])).toEqual({
			segments: [],
			loose: ["a", "b"],
		});
	});
});

describe("draftKeys and filled", () => {
	it("lists the files in the text and in the strip", () => {
		expect([...draftKeys({ segments: [text("a "), file("x")], loose: ["y"] })]).toEqual([
			"x",
			"y",
		]);
	});

	it("counts a file in the strip as something to send", () => {
		expect(filled(EMPTY_DRAFT)).toBe(false);
		expect(filled({ segments: [text(" \n ")], loose: [] })).toBe(false);
		expect(filled({ segments: [], loose: ["a"] })).toBe(true);
		expect(filled({ segments: [text("hi")], loose: [] })).toBe(true);
	});
});

describe("withRestored", () => {
	it("puts what comes back before what you typed, with a blank line", () => {
		expect(
			withRestored(
				{ segments: [text("first")], loose: ["a"] },
				{ segments: [text("second")], loose: ["b", "a"] }
			)
		).toEqual({ segments: [text("first\n\nsecond")], loose: ["a", "b"] });
	});

	it("takes the place of a blank box", () => {
		expect(
			withRestored(
				{ segments: [text("first")], loose: [] },
				{ segments: [text("  ")], loose: [] }
			)
		).toEqual({ segments: [text("first")], loose: [] });
	});
});

describe("queued messages", () => {
	const excerpt = ref("att-excerpt", "excerpt.md");
	const message = queued(
		`fix ${M} like #12 ${M}`,
		[ref("att-1"), excerpt, ref("att-2")],
		[
			{
				type: "agent",
				agent: "agent-3",
				title: "Usage meter",
				vendor: "codex",
				device: "device-1",
				excerpt: "att-excerpt",
			},
			{ type: "attachment", id: "att-1" },
			pr,
		]
	);

	it("goes again as it went", () => {
		expect(queuedInput(message)).toEqual({
			text: message.text,
			attachments: ["att-1", "att-excerpt", "att-2"],
			refs: message.refs,
		});
		expect(queuedInput(queued("hi", []))).toEqual({ text: "hi", attachments: [] });
	});

	it("brings back its files but not an agent's excerpt", () => {
		expect(queuedFiles(message).map((file) => file.id)).toEqual(["att-1", "att-2"]);
	});

	it("comes back as a draft, with the files at its end in the strip", () => {
		const restored = queuedDraft(message, (id) => `key-${id}`);
		expect(restored.draft).toEqual({
			segments: [
				text("fix "),
				{
					type: "pill",
					pill: {
						type: "agent",
						agent: "agent-3",
						title: "Usage meter",
						vendor: "codex",
						device: "device-1",
						excerpt: null,
					},
				},
				text(" like #12"),
			],
			loose: ["key-att-1", "key-att-2"],
		});
		expect([...restored.gitItems.values()]).toEqual([pr]);
	});
});

describe("saved drafts", () => {
	it("keep the pills, the strip and the files they hold", () => {
		const draft = { segments: [text("see "), file("a"), text(" #12")], loose: ["b"] };
		const files = [staged("a"), staged("b"), staged("gone")];
		const fragment = fragmentOf(draft, files, "device-1", [pr]);
		expect(fragment.files.map((saved) => saved.key)).toEqual(["a", "b"]);
		expect(fragment.files[0]).toMatchObject({ device: "device-1", ref: files[0]!.ref });
		const back = draftOfFragment(fragment);
		expect(back.draft).toEqual(draft);
		expect([...back.gitItems.values()]).toEqual([pr]);
	});
});

describe("sending", () => {
	it("names each file that isn't ready", () => {
		const whole = [text("see "), file("a"), text(" "), file("b"), text(" "), file("c")];
		const files = [
			staged("a"),
			staged("b", { state: "staging", ref: null }),
			staged("c", { state: "failed", ref: null, error: "It was deleted." }),
			staged("d", { state: "failed", ref: null }),
		];
		expect(unreadyFiles(whole, files)).toEqual([
			{ name: "b.png", error: null },
			{ name: "c.png", error: "It was deleted." },
		]);
	});

	it("sends the desktop's wire format, with the strip's files last", () => {
		const whole = wholeOf({
			segments: [text("compare "), file("a"), text(" with #12  ")],
			loose: ["b"],
		});
		const input = messageInput(
			whole,
			[pr],
			[staged("a"), staged("b")],
			{
				excerpts: new Map(),
				notes: new Map(),
			},
			true
		);
		expect(input).toEqual({
			text: `compare ${M} with #12 ${M}`,
			attachments: ["att-a", "att-b"],
			refs: [{ type: "attachment", id: "att-a" }, { type: "attachment", id: "att-b" }, pr],
		});
	});

	it("adds the excerpt of an agent on another device", () => {
		const whole: Segment[] = [
			text("ask "),
			{
				type: "pill",
				pill: {
					type: "agent",
					agent: "agent-3",
					title: "Usage meter",
					vendor: "codex",
					device: "device-2",
					excerpt: null,
				},
			},
		];
		const input = messageInput(
			whole,
			[],
			[],
			{ excerpts: new Map([["agent-3", "att-x"]]), notes: new Map() },
			true
		);
		expect(input.attachments).toEqual(["att-x"]);
		expect(input.refs).toEqual([expect.objectContaining({ type: "agent", excerpt: "att-x" })]);
	});

	it("sends plain text the way it always went", () => {
		expect(
			messageInput(
				[text("hi there \n")],
				[],
				[],
				{ excerpts: new Map(), notes: new Map() },
				true
			)
		).toEqual({ text: "hi there", attachments: [] });
	});
});
