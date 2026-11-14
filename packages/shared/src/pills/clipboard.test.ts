// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import {
	type ClipboardFragment,
	decodeClipboardHtml,
	decodeFragment,
	DRAFT_CLIPBOARD_MIME,
	encodeClipboardHtml,
	encodeFragment,
	readFragment,
} from "./clipboard";
import type { AgentRef, GitItemRef, NoteRef } from "./draft";

const fragment: ClipboardFragment = {
	version: 1,
	origin: "composer-1",
	segments: [
		{ type: "text", text: "Look <here> & " },
		{ type: "pill", pill: { type: "attachment", key: "chip-1" } },
	],
	files: [
		{
			key: "chip-1",
			name: "shot.png",
			kind: "image",
			source: "/Users/someone/shot.png",
			ref: {
				id: "att-shot",
				name: "shot.png",
				kind: "image",
				bytes: 10,
				mime: "image/png",
				path: "/staged/shot.png",
				thumbnail: null,
			},
			device: "device-1",
		},
	],
	gitItems: [],
};

const agent: AgentRef = {
	type: "agent",
	agent: "agent-2",
	title: "Big picture check",
	vendor: "claude",
	device: "device-1",
	excerpt: null,
};

const pr: GitItemRef = {
	type: "git_item",
	repo: "owner/repo",
	number: 7,
	kind: "pull_request",
	title: "Fix the meter",
	state: "open",
	url: "https://github.com/owner/repo/pull/7",
	head: "meter",
	base: "main",
	body: "",
	checked_at: 1_793_361_600_000,
};

const data = (types: Record<string, string>) => ({ getData: (type: string) => types[type] ?? "" });

/** The fragment with its one file changed. */
const withFile = (change: Record<string, unknown>) =>
	JSON.stringify({ ...fragment, files: [{ ...fragment.files[0], ...change }] });

describe("the draft clipboard", () => {
	it("round trips through its own type", () => {
		const encoded = encodeFragment(fragment)!;
		expect(decodeFragment(encoded)).toEqual(fragment);
		expect(readFragment(data({ [DRAFT_CLIPBOARD_MIME]: encoded }))).toEqual(fragment);
	});

	it("comes back from HTML, which shows the plain text, with its files as markers", () => {
		const html = encodeClipboardHtml(
			"Look <here> & [Image 1: shot.png]",
			encodeFragment(fragment)!
		);
		expect(html).toContain("Look &lt;here&gt; &amp; [Image 1: shot.png]");
		const fromHtml: ClipboardFragment = {
			...fragment,
			segments: [{ type: "text", text: "Look <here> & [Image 1: shot.png]" }],
			files: [],
		};
		expect(decodeClipboardHtml(html)).toEqual(fromHtml);
		expect(readFragment(data({ "text/html": html }))).toEqual(fromHtml);
	});

	it("prefers its own type to the HTML beside it", () => {
		const encoded = encodeFragment(fragment)!;
		const html = encodeClipboardHtml("Look", encoded);
		expect(readFragment(data({ [DRAFT_CLIPBOARD_MIME]: encoded, "text/html": html }))).toEqual(
			fragment
		);
	});

	it("keeps no file from HTML another app wrote, but keeps its other pills", () => {
		const forged: ClipboardFragment = {
			version: 1,
			origin: "page",
			segments: [
				{ type: "text", text: "see " },
				{ type: "pill", pill: { type: "attachment", key: "k1" } },
				{ type: "text", text: " and " },
				{ type: "pill", pill: { type: "attachment", key: "k2" } },
				{ type: "pill", pill: { type: "attachment", key: "k3" } },
				{ type: "text", text: ", ask " },
				{ type: "pill", pill: agent },
				{ type: "text", text: " about #7" },
			],
			files: [
				{
					key: "k1",
					name: "id_rsa",
					kind: "file",
					source: "/Users/someone/.ssh/id_rsa",
					ref: null,
					device: null,
				},
				{
					key: "k2",
					name: "shot.png",
					kind: "image",
					source: null,
					ref: null,
					device: null,
					found: {
						agent: "agent-1",
						device: "device-1",
						path: "/home/someone/.ssh/id_rsa",
					},
				},
			],
			gitItems: [pr],
		};
		const html = `<meta charset="utf-8"><pre data-maxhq-draft="${encodeURIComponent(
			JSON.stringify(forged)
		)}">see</pre>`;
		const read = readFragment(data({ "text/html": html }));
		expect(read?.files).toEqual([]);
		expect(read?.segments).toEqual([
			{ type: "text", text: "see [File: id_rsa] and [Image 1: shot.png], ask " },
			{ type: "pill", pill: agent },
			{ type: "text", text: " about #7" },
		]);
		expect(read?.gitItems).toEqual([pr]);
	});

	it("keeps a note's pill", () => {
		const note: NoteRef = {
			type: "note",
			project: "proj-1",
			note: "note-1",
			title: "GTFS release steps",
			copy: null,
		};
		const copied: ClipboardFragment = {
			...fragment,
			segments: [{ type: "pill", pill: note }],
			files: [],
		};
		expect(decodeFragment(encodeFragment(copied)!)).toEqual(copied);
		const broken = { ...copied, segments: [{ type: "pill", pill: { ...note, note: "" } }] };
		expect(decodeFragment(JSON.stringify(broken))).toBeNull();
	});

	it("refuses anything else", () => {
		expect(decodeFragment("not json")).toBeNull();
		expect(decodeFragment(JSON.stringify({ ...fragment, version: 2 }))).toBeNull();
		expect(
			decodeFragment(
				JSON.stringify({
					...fragment,
					segments: [{ type: "pill", pill: { type: "page" } }],
				})
			)
		).toBeNull();
		expect(decodeClipboardHtml("<p>plain</p>")).toBeNull();
		expect(readFragment(data({ "text/plain": "hello" }))).toBeNull();
	});
});

describe("where an agent can see a copied file", () => {
	const found = { agent: "agent-1", device: "device-1", path: "/work/attachments/a/shot.png" };

	it("comes back through its own type", () => {
		expect(decodeFragment(withFile({ found }))?.files[0]?.found).toEqual(found);
		expect(decodeFragment(withFile({ found: null }))?.files[0]?.found).toBeNull();
	});

	it("may be left out, as older copies do", () => {
		expect(decodeFragment(withFile({}))).toEqual(fragment);
	});

	it("refuses a file whose place is malformed", () => {
		for (const bad of [
			"/work/shot.png",
			42,
			[],
			{},
			{ ...found, agent: "" },
			{ ...found, device: 7 },
			{ ...found, path: null },
			{ agent: "agent-1", device: "device-1" },
		])
			expect(decodeFragment(withFile({ found: bad }))).toBeNull();
	});
});
