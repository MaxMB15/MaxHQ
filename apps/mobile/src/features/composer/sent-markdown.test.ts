// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import type { AttachmentRef, InlineRef } from "@maxhq/shared/bindings";
import { OBJECT_MARK } from "@maxhq/shared/pills/draft";
import { sentMessage } from "@maxhq/shared/pills/sent";
import { describe, expect, it } from "vitest";

import { pillIndex, sentMarkdown, tapOf } from "./sent-markdown";

const M = OBJECT_MARK;

const file = (id: string, name: string, kind: AttachmentRef["kind"]): AttachmentRef => ({
	id,
	name,
	kind,
	bytes: 10,
	mime: null,
	path: `/staged/${name}`,
	thumbnail: null,
});

const agent: InlineRef = {
	type: "agent",
	agent: "agent-3",
	title: "Usage meter",
	vendor: "codex",
	device: "device-1",
	excerpt: null,
};

const note: InlineRef = {
	type: "note",
	project: "project-1",
	note: "note-1",
	title: "Plan [draft]",
	copy: null,
};

const quote: InlineRef = {
	type: "quote",
	agent: "agent-2",
	agent_title: "Fix login",
	item: "item-9",
	text: "the cache misses",
	position: 0,
	source: "reply",
	from: null,
};

const pr: InlineRef = {
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

const markdownOf = (
	text: string,
	refs: InlineRef[],
	attachments: AttachmentRef[] = [],
	titleOf?: (agent: string) => string | undefined
) => {
	const sent = sentMessage({ text, refs, attachments });
	return { sent, markdown: sentMarkdown(sent.pieces, sent.factsOf, sent.numbers, titleOf) };
};

describe("sentMarkdown", () => {
	it("links each pill the phone opens", () => {
		const { markdown } = markdownOf(`ask ${M} about ${M} and #12`, [agent, note, pr]);
		expect(markdown).toBe(
			"ask [Usage meter](maxhq-pill:agent/1) about [Plan \\[draft\\]](maxhq-pill:note/3) and " +
				"[\\#12 Cache usage](maxhq-pill:git/5)"
		);
	});

	it("names an agent by its title now", () => {
		const { markdown } = markdownOf(`ask ${M}`, [agent], [], () => "Usage meter v2");
		expect(markdown).toBe("ask [Usage meter v2](maxhq-pill:agent/1)");
	});

	it("links a picture but leaves other files and quotes as text", () => {
		const shot = file("att-1", "shot.png", "image");
		const spec = file("att-2", "spec.pdf", "pdf");
		const { markdown, sent } = markdownOf(
			`${M} ${M} ${M}`,
			[{ type: "attachment", id: "att-1" }, { type: "attachment", id: "att-2" }, quote],
			[shot, spec]
		);
		expect(tapOf(sent.pieces[0]!, sent.factsOf)).toBe("image");
		expect(markdown).toBe(
			'[shot\\.png](maxhq-pill:image/0) \\[PDF 1\\: spec\\.pdf\\] \\"the cache misses\\"'
		);
	});

	it("shows a pill inside code as typed", () => {
		const { markdown } = markdownOf(`run \`${M}\` then ${M}`, [agent, agent]);
		expect(markdown).toBe("run `[Agent: Usage meter]` then [Usage meter](maxhq-pill:agent/3)");
	});

	it("leaves a message with no pills as it is", () => {
		expect(markdownOf("**fix** the cache", []).markdown).toBe("**fix** the cache");
	});
});

describe("pillIndex", () => {
	it("reads the piece a pill's link names", () => {
		expect(pillIndex("maxhq-pill:agent/3")).toBe(3);
		expect(pillIndex("https://example.com/3")).toBeNull();
		expect(pillIndex("maxhq-pill:agent/")).toBeNull();
	});
});
