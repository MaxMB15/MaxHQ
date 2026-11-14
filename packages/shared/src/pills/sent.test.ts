// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import {
	type AgentRef,
	type FileFacts,
	type GitItemRef,
	OBJECT_MARK,
	type QuoteRef,
} from "./draft";
import type { AttachmentRef } from "../bindings";
import { decodeFragment, encodeFragment } from "./clipboard";
import {
	messageFragment,
	pieceText,
	sentFragment,
	sentMessage,
	sentNumbers,
	sentPieces,
	sentText,
} from "./sent";

const M = OBJECT_MARK;

const quote: QuoteRef = {
	type: "quote",
	agent: "agent-2",
	agent_title: "Fix login",
	item: "item-9",
	text: "the cache misses on the second poll",
	position: 0,
	source: "reply",
	from: null,
};

const agent: AgentRef = {
	type: "agent",
	agent: "agent-3",
	title: "Usage [meter]",
	vendor: "codex",
	device: "device-1",
	excerpt: "att-excerpt",
};

const pr = (number: number, title = "Cache usage between polls"): GitItemRef => ({
	type: "git_item",
	repo: "owner/repo",
	number,
	kind: "pull_request",
	title,
	state: "open",
	url: `https://github.com/owner/repo/pull/${number}`,
	head: "usage-cache",
	base: "main",
	body: "",
	checked_at: 1_793_361_600_000,
});

const file = (id: string) => ({ type: "attachment" as const, id });

const facts: Record<string, FileFacts> = {
	"att-shot": { name: "shot.png", kind: "image" },
	"att-spec": { name: "spec.pdf", kind: "pdf" },
	"att-paste": { name: "pasted-image.png", kind: "image" },
	"att-log": { name: "player.log", kind: "file" },
};
const factsOf = (id: string) => facts[id];

describe("sentPieces", () => {
	it("puts each placed ref where its mark sits, in order", () => {
		expect(
			sentPieces({
				text: `Compare ${M} with ${M}, ask ${M}`,
				refs: [file("att-shot"), quote, agent],
			})
		).toEqual([
			{ type: "text", text: "Compare " },
			{ type: "pill", ref: file("att-shot") },
			{ type: "text", text: " with " },
			{ type: "pill", ref: quote },
			{ type: "text", text: ", ask " },
			{ type: "pill", ref: agent },
		]);
	});

	it("shows each #N that resolved as its pull request, and leaves the rest as text", () => {
		expect(
			sentPieces({
				text: `See #412 and #7, then ${M} in \`#412\` and #412.`,
				refs: [pr(412), file("att-shot")],
			})
		).toEqual([
			{ type: "text", text: "See " },
			{ type: "git", text: "#412", ref: pr(412) },
			{ type: "text", text: " and #7, then " },
			{ type: "pill", ref: file("att-shot") },
			{ type: "text", text: " in `#412` and " },
			{ type: "git", text: "#412", ref: pr(412) },
			{ type: "text", text: "." },
		]);
	});

	it("takes the first ref when two name one number", () => {
		const pieces = sentPieces({ text: "#412", refs: [pr(412), pr(412, "Another title")] });
		expect(pieces).toEqual([{ type: "git", text: "#412", ref: pr(412) }]);
	});

	it("tells the same number in another repo apart", () => {
		const other = { ...pr(412, "Elsewhere"), repo: "someone/else" };
		const pieces = sentPieces({ text: "Someone/Else#412 and #412", refs: [pr(412), other] });
		expect(pieces).toEqual([
			{ type: "git", text: "Someone/Else#412", ref: other },
			{ type: "text", text: " and " },
			{ type: "git", text: "#412", ref: pr(412) },
		]);
	});

	it("drops a mark with no ref, and puts a ref with no mark at the end", () => {
		expect(sentPieces({ text: `One ${M}${M} two`, refs: [quote] })).toEqual([
			{ type: "text", text: "One " },
			{ type: "pill", ref: quote },
			{ type: "text", text: " two" },
		]);
		expect(sentPieces({ text: `Look${M}`, refs: [file("att-shot"), quote, agent] })).toEqual([
			{ type: "text", text: "Look" },
			{ type: "pill", ref: file("att-shot") },
			{ type: "text", text: " " },
			{ type: "pill", ref: quote },
			{ type: "text", text: " " },
			{ type: "pill", ref: agent },
		]);
	});

	it("keeps a message with no refs as one run of text", () => {
		expect(sentPieces({ text: "Ship it, #412 too" })).toEqual([
			{ type: "text", text: "Ship it, #412 too" },
		]);
		expect(sentPieces({ text: "" })).toEqual([]);
	});
});

describe("sentNumbers", () => {
	it("numbers placed images and PDFs per kind in text order, once each", () => {
		const pieces = sentPieces({
			text: `${M} ${M} ${M} ${M} ${M}`,
			refs: [
				file("att-spec"),
				file("att-shot"),
				file("att-log"),
				file("att-shot"),
				file("att-paste"),
			],
		});
		expect([...sentNumbers(pieces, factsOf)]).toEqual([
			["att-spec", 1],
			["att-shot", 1],
			["att-paste", 2],
		]);
	});
});

describe("sentText", () => {
	it("writes each pill the way another app can read it", () => {
		const pieces = sentPieces({
			text: `${M} and ${M} said ${M}, ask ${M} about #412 and ${M}`,
			refs: [file("att-shot"), file("att-paste"), quote, agent, pr(412), file("att-gone")],
		});
		expect(sentText(pieces, factsOf)).toBe(
			'[Image 1: shot.png] and [Image 2: pasted-image.png] said "the cache misses on the second poll", ask [Agent: Usage meter] about #412 and [File: file]'
		);
	});

	it("gives one piece on its own the same words", () => {
		const pieces = sentPieces({
			text: `${M} ${M}`,
			refs: [file("att-shot"), file("att-spec")],
		});
		const numbers = sentNumbers(pieces, factsOf);
		expect(pieces.map((piece) => pieceText(piece, factsOf, numbers))).toEqual([
			"[Image 1: shot.png]",
			" ",
			"[PDF 1: spec.pdf]",
		]);
	});
});

describe("sentMessage", () => {
	const shot: AttachmentRef = {
		id: "att-shot",
		name: "shot.png",
		kind: "image",
		bytes: 2048,
		mime: "image/png",
		path: "/home/user/shot.png",
		thumbnail: null,
	};
	const excerpt: AttachmentRef = { ...shot, id: "att-excerpt", name: "excerpt.md", kind: "file" };

	it("numbers its files and looks them up by ID", () => {
		const sent = sentMessage({
			text: `See ${M} here`,
			refs: [file("att-shot")],
			attachments: [shot],
		});
		expect(sent.factsOf("att-shot")).toBe(shot);
		expect([...sent.numbers]).toEqual([["att-shot", 1]]);
		expect(sentText(sent.pieces, sent.factsOf)).toBe("See [Image 1: shot.png] here");
	});

	it("shows the files you sent, not an agent pill's excerpt", () => {
		const sent = sentMessage({
			text: `Look ${M}`,
			refs: [{ ...agent, title: "Other" }],
			attachments: [shot, excerpt],
		});
		expect(sent.shown).toEqual([shot]);
		expect(sentText(sent.pieces, sent.factsOf)).toBe("Look [Agent: Other]");
	});
});

describe("sentFragment", () => {
	const fileOf = (id: string): AttachmentRef | undefined => {
		const known = facts[id];
		if (!known) return undefined;
		const { name, kind } = known;
		return {
			id,
			name,
			kind: kind === "missing" ? "file" : kind,
			bytes: 10,
			mime: null,
			path: `/work/attachments/agent-1/${id}/${name}`,
			thumbnail: null,
		};
	};
	const place = { agent: "agent-1", device: "device-1" };

	it("gives a composer the pills back, each file named where the agent has it", () => {
		const pieces = sentPieces({
			text: `Look at ${M} and ${M}, ask ${M} about #412 and #412`,
			refs: [file("att-shot"), quote, agent, pr(412)],
		});
		const fragment = sentFragment(pieces, fileOf, place);
		expect(fragment).toEqual({
			version: 1,
			origin: "sent",
			segments: [
				{ type: "text", text: "Look at " },
				{ type: "pill", pill: { type: "attachment", key: "sent-att-shot" } },
				{ type: "text", text: " and " },
				{ type: "pill", pill: quote },
				{ type: "text", text: ", ask " },
				// The excerpt went with that send, so the next one takes its own.
				{ type: "pill", pill: { ...agent, excerpt: null } },
				{ type: "text", text: " about #412 and #412" },
			],
			files: [
				{
					key: "sent-att-shot",
					name: "shot.png",
					kind: "image",
					source: null,
					ref: null,
					device: null,
					found: {
						agent: "agent-1",
						device: "device-1",
						path: "/work/attachments/agent-1/att-shot/shot.png",
					},
				},
			],
			gitItems: [pr(412)],
		});
		expect(decodeFragment(encodeFragment(fragment))).toEqual(fragment);
	});

	it("gives a note's pill back without its copy, and writes it as its marker", () => {
		const note = {
			type: "note" as const,
			project: "proj-2",
			note: "note-1",
			title: "GTFS release steps",
			copy: "att-copy",
		};
		const pieces = sentPieces({ text: `Follow ${M}`, refs: [note] });
		expect(sentFragment(pieces, fileOf, place).segments).toEqual([
			{ type: "text", text: "Follow " },
			{ type: "pill", pill: { ...note, copy: null } },
		]);
		expect(sentText(pieces, factsOf)).toBe("Follow [Note: GTFS release steps]");
	});

	it("gives a visual's pill back with its page, and writes it as its marker", () => {
		const visual = {
			type: "visual" as const,
			attachment: "att-log",
			title: "Weekly [usage]",
			from: null,
		};
		const pieces = sentPieces({ text: `Thinner bars on ${M}`, refs: [visual] });
		const fragment = sentFragment(pieces, fileOf, place);
		expect(fragment.segments).toEqual([
			{ type: "text", text: "Thinner bars on " },
			{
				type: "pill",
				pill: { type: "visual", key: "sent-att-log", title: "Weekly [usage]", from: null },
			},
		]);
		expect(fragment.files.map((each) => each.found?.path)).toEqual([
			"/work/attachments/agent-1/att-log/player.log",
		]);
		expect(sentText(pieces, factsOf)).toBe("Thinner bars on [Visual: Weekly usage]");
	});

	it("keeps the text it's given, cut where the selection cut it", () => {
		const fragment = sentFragment(
			[
				{ type: "text", text: "ok at " },
				{ type: "pill", ref: file("att-shot") },
			],
			fileOf,
			place
		);
		expect(fragment.segments).toEqual([
			{ type: "text", text: "ok at " },
			{ type: "pill", pill: { type: "attachment", key: "sent-att-shot" } },
		]);
	});

	it("names no place for a file that's gone, or when the device isn't known", () => {
		const pieces = sentPieces({
			text: `${M} ${M}`,
			refs: [file("att-gone"), file("att-log")],
		});
		const gone = sentFragment(pieces, fileOf, place).files[0];
		expect(gone).toMatchObject({ name: "file", kind: "missing", found: null });
		const unknown = sentFragment(pieces, fileOf, { agent: "agent-1", device: null });
		expect(unknown.files.map((each) => each.found)).toEqual([null, null]);
	});
});

describe("messageFragment", () => {
	const sentFile = (id: string, name: string): AttachmentRef => ({
		id,
		name,
		kind: "file",
		bytes: 10,
		mime: null,
		path: `/work/attachments/agent-1/${id}/${name}`,
		thumbnail: null,
	});
	const shot = { ...sentFile("att-shot", "shot.png"), kind: "image" as const };
	const log = sentFile("att-log", "player.log");
	const place = { agent: "agent-1", device: "device-1" };

	it("gives the whole message back, with a file it placed nowhere at the end", () => {
		const fragment = messageFragment(
			{
				text: `Look at ${M}, ask ${M} about #412`,
				attachments: [shot, log, sentFile("att-excerpt", "excerpt.md")],
				refs: [file("att-shot"), agent, pr(412)],
			},
			place
		);
		expect(fragment.segments).toEqual([
			{ type: "text", text: "Look at " },
			{ type: "pill", pill: { type: "attachment", key: "sent-att-shot" } },
			{ type: "text", text: ", ask " },
			// The excerpt went with that send, so the next one takes its own.
			{ type: "pill", pill: { ...agent, excerpt: null } },
			{ type: "text", text: " about #412 " },
			{ type: "pill", pill: { type: "attachment", key: "sent-att-log" } },
		]);
		expect(fragment.files.map(({ key, found }) => [key, found?.path])).toEqual([
			["sent-att-shot", shot.path],
			["sent-att-log", log.path],
		]);
		expect(fragment.gitItems).toEqual([pr(412)]);
	});

	it("names no place for a file the message no longer lists", () => {
		const fragment = messageFragment(
			{ text: `${M}`, attachments: [], refs: [file("att-gone")] },
			place
		);
		expect(fragment.files).toEqual([
			expect.objectContaining({ key: "sent-att-gone", kind: "missing", found: null }),
		]);
	});
});
