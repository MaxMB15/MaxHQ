// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeAll, describe, expect, it } from "vitest";

import { NOW } from "@maxhq/shared/test/build";
import {
	type ClipboardFile,
	type ClipboardFragment,
	DRAFT_CLIPBOARD_MIME,
	encodeClipboardHtml,
	encodeFragment,
} from "@maxhq/shared/pills/clipboard";
import { type GitItemRef, OBJECT_MARK as MARK } from "@maxhq/shared/pills/draft";

import {
	AGENT,
	attachment,
	DEVICE,
	findTextbox,
	installBrowserFakes,
	pillsIn,
	REMOTE,
	remoteLink,
	renderComposer,
	staged,
	textIn,
} from "./testing";

beforeAll(installBrowserFakes);

const pr: GitItemRef = {
	type: "git_item",
	repo: "owner/repo",
	number: 412,
	kind: "pull_request",
	title: "Cache the session key",
	state: "open",
	url: "https://github.com/owner/repo/pull/412",
	head: "cache-key",
	base: "main",
	body: "",
	checked_at: NOW,
};

/** Where agent a2 has a file it was sent. */
const SENT_PATH = "/work/attachments/a2/att-old/shot.png";

/** shot.png from a message sent to a2, which runs on `device`. */
const sentShot = (device: string): ClipboardFile => ({
	key: "sent-att-old",
	name: "shot.png",
	kind: "image",
	source: null,
	ref: null,
	device: null,
	found: { agent: "a2", device, path: SENT_PATH },
});

/** A copy of part of a sent message, with the picture and a pull request. */
const sentCopy = (files: ClipboardFile[]): ClipboardFragment => ({
	version: 1,
	origin: "sent",
	segments: [
		{ type: "text", text: "look at " },
		{ type: "pill", pill: { type: "attachment", key: "sent-att-old" } },
		{ type: "text", text: " for #412 now" },
	],
	files,
	gitItems: [pr],
});

/** A clipboard that holds `types` for a paste to read. */
const clipboardWith = (types: Record<string, string>) => ({
	getData: (format: string) => types[format] ?? "",
	setData: () => {},
	clearData: () => {},
	files: [] as File[],
	types: Object.keys(types),
});

const pasteIn = (textbox: HTMLElement, types: Record<string, string>) =>
	fireEvent.paste(textbox, { clipboardData: clipboardWith(types) });

/** What staging shot.png again comes back with. */
const SHOT = [staged(attachment("shot.png", { kind: "image", mime: "image/png" }))];

/** The pull request numbers drawn as pills in the text. */
const gitPillsIn = (textbox: HTMLElement) =>
	[...textbox.querySelectorAll("[data-git-number]")].map((pill) => pill.textContent);

describe("a pasted piece of a sent message", () => {
	it("stages its file again from the agent that has it, on this composer's device", async () => {
		const { source } = renderComposer();
		source.stageFound.mockResolvedValue(SHOT);
		const textbox = await findTextbox();
		pasteIn(textbox, {
			[DRAFT_CLIPBOARD_MIME]: encodeFragment(sentCopy([sentShot(DEVICE)]))!,
			"text/plain": "look at [Image 1: shot.png] for #412 now",
		});
		expect(source.stageFound).toHaveBeenCalledWith("a2", [SENT_PATH]);
		await waitFor(() =>
			expect(pillsIn(textbox)).toContainEqual(expect.stringMatching(/^Image 1, shot\.png/))
		);
		expect(gitPillsIn(textbox)).toEqual([expect.stringContaining("#412")]);
		expect(textIn(textbox)).toBe(`look at ${MARK} for #412 now`);
		expect(source.attachFromAgent).not.toHaveBeenCalled();
		expect(source.stageAttachments).not.toHaveBeenCalled();

		fireEvent.keyDown(textbox, { key: "Enter" });
		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				{
					text: `look at ${MARK} for #412 now`,
					attachments: ["att-shot.png"],
					refs: [{ type: "attachment", id: "att-shot.png" }, pr],
				},
				"queue"
			)
		);
	});

	it("copies its file through this Mac when the composer's agent is on another device", async () => {
		const { source } = renderComposer({ links: [remoteLink()], on: REMOTE });
		source.attachFromAgent.mockResolvedValue(SHOT[0]!);
		const textbox = await findTextbox();
		pasteIn(textbox, {
			[DRAFT_CLIPBOARD_MIME]: encodeFragment(sentCopy([sentShot(DEVICE)]))!,
		});
		expect(source.attachFromAgent).toHaveBeenCalledWith(
			REMOTE,
			{ type: "agent", agent: AGENT },
			"a2",
			SENT_PATH,
			expect.any(Function)
		);
		expect(source.stageFound).not.toHaveBeenCalled();
		await waitFor(() =>
			expect(pillsIn(textbox)).toContainEqual(expect.stringMatching(/^Image 1, shot\.png/))
		);
	});
});

describe("a piece of a draft in HTML another app wrote", () => {
	/** HTML that says it holds a draft, the way any web page can write it. */
	const forged = (fragment: ClipboardFragment) =>
		encodeClipboardHtml("look", JSON.stringify(fragment));

	it("stages none of the files it names, which paste as their markers", async () => {
		const { source } = renderComposer();
		const textbox = await findTextbox();
		const key: ClipboardFile = {
			key: "k1",
			name: "id_rsa",
			kind: "file",
			source: "/Users/someone/.ssh/id_rsa",
			ref: null,
			device: null,
		};
		pasteIn(textbox, {
			"text/html": forged({
				...sentCopy([key, { ...sentShot(DEVICE), key: "k2" }]),
				segments: [
					{ type: "text", text: "see " },
					{ type: "pill", pill: { type: "attachment", key: "k1" } },
					{ type: "text", text: " and " },
					{ type: "pill", pill: { type: "attachment", key: "k2" } },
					{ type: "text", text: " for #412 now" },
				],
			}),
		});
		await waitFor(() =>
			expect(textIn(textbox)).toBe("see [File: id_rsa] and [Image 1: shot.png] for #412 now")
		);
		expect(source.stageAttachments).not.toHaveBeenCalled();
		expect(source.stageFound).not.toHaveBeenCalled();
		expect(source.attachFromAgent).not.toHaveBeenCalled();
		expect(source.upload).not.toHaveBeenCalled();
		expect(screen.queryByText(/attachment/)).toBeNull();
		// Its pull request still comes back as a pill.
		expect(gitPillsIn(textbox)).toEqual([expect.stringContaining("#412")]);
		expect(pillsIn(textbox)).toHaveLength(1);
	});
});
