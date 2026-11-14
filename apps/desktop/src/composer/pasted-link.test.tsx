// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, waitFor } from "@testing-library/react";
import { beforeAll, describe, expect, it } from "vitest";

import type { GitItemSearch } from "@maxhq/shared/data/source";
import type { GitItemRef } from "@maxhq/shared/pills/draft";
import { NOW } from "@maxhq/shared/test/build";

import {
	AGENT,
	deferred,
	editorOf,
	findTextbox,
	installBrowserFakes,
	renderComposer,
	textIn,
	type,
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

const other: GitItemRef = {
	...pr,
	repo: "someone/else",
	number: 7,
	kind: "issue",
	title: "Crash on start",
	url: "https://github.com/someone/else/issues/7",
	head: null,
	base: null,
};

const LINK = "https://github.com/owner/repo/pull/412/files#diff-1";
const OTHER_LINK = "https://github.com/someone/else/issues/7";

/** What the agent's own repo, owner/repo, has for a number. */
const own = (items: GitItemRef[]): GitItemSearch => ({
	repo: "owner/repo",
	items,
	unavailable: null,
});

const pasteText = (textbox: HTMLElement, text: string) =>
	fireEvent.paste(textbox, {
		clipboardData: {
			getData: (format: string) => (format === "text/plain" ? text : ""),
			setData: () => {},
			clearData: () => {},
			files: [] as File[],
			types: ["text/plain"],
		},
	});

const gitPillsIn = (textbox: HTMLElement) =>
	[...textbox.querySelectorAll("[data-git-number]")].map((pill) => pill.textContent);

/** Lets the lookups that already answered finish. */
const settle = () => act(async () => {});

describe("a pasted link to a pull request or issue", () => {
	it("becomes its number's pill in the agent's repo, and goes out as a ref", async () => {
		const { source } = renderComposer();
		source.findGitItems.mockResolvedValue(own([pr]));
		const textbox = await findTextbox();
		await type(textbox, "See ");
		pasteText(textbox, ` ${LINK}\n`);

		await waitFor(() => expect(gitPillsIn(textbox)).toEqual(["#412"]));
		expect(textIn(textbox)).toBe("See #412");
		expect(source.findGitItems).toHaveBeenCalledTimes(1);
		expect(source.findGitItems).toHaveBeenCalledWith(AGENT, "412");

		await type(textbox, " now");
		fireEvent.keyDown(textbox, { key: "Enter" });
		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				{ text: "See #412 now", attachments: [], refs: [pr] },
				"queue"
			)
		);
	});

	it("becomes owner/repo#N when it's in another repo, which it looks in", async () => {
		const { source } = renderComposer();
		source.findGitItems.mockImplementation(async (_agent, _query, repo) =>
			repo === "someone/else"
				? { repo, items: [other], unavailable: null }
				: own([{ ...pr, number: 7 }])
		);
		const textbox = await findTextbox();
		pasteText(textbox, OTHER_LINK);

		await waitFor(() => expect(gitPillsIn(textbox)).toEqual(["someone/else#7"]));
		expect(textIn(textbox)).toBe("someone/else#7");
		expect(source.findGitItems).toHaveBeenCalledWith(AGENT, "7", "someone/else");

		fireEvent.keyDown(textbox, { key: "Enter" });
		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				{ text: "someone/else#7", attachments: [], refs: [other] },
				"queue"
			)
		);
	});

	it("goes in at once, so typing after it carries on while it looks up", async () => {
		const { source } = renderComposer();
		const lookup = deferred<GitItemSearch>();
		source.findGitItems.mockReturnValue(lookup.promise);
		const textbox = await findTextbox();
		pasteText(textbox, LINK);
		expect(textIn(textbox)).toBe(LINK);

		await type(textbox, " please");
		await act(async () => lookup.resolve(own([pr])));
		await waitFor(() => expect(gitPillsIn(textbox)).toEqual(["#412"]));
		expect(textIn(textbox)).toBe("#412 please");
		expect(editorOf(textbox).state.selection.from).toBe("#412 please".length + 1);
	});

	it("stays text when the lookup fails", async () => {
		const { source } = renderComposer();
		source.findGitItems.mockRejectedValue(new Error("No network"));
		const textbox = await findTextbox();
		pasteText(textbox, LINK);

		await waitFor(() => expect(source.findGitItems).toHaveBeenCalledTimes(2));
		await settle();
		expect(textIn(textbox)).toBe(LINK);
		expect(gitPillsIn(textbox)).toEqual([]);
	});

	it("stays text when the lookup doesn't find it", async () => {
		const { source } = renderComposer();
		const textbox = await findTextbox();
		pasteText(textbox, OTHER_LINK);

		await waitFor(() =>
			expect(source.findGitItems).toHaveBeenCalledWith(AGENT, "7", "someone/else")
		);
		await settle();
		expect(textIn(textbox)).toBe(OTHER_LINK);
		expect(gitPillsIn(textbox)).toEqual([]);

		fireEvent.keyDown(textbox, { key: "Enter" });
		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				{ text: OTHER_LINK, attachments: [] },
				"queue"
			)
		);
	});

	it("stays as you left it when you change it before the lookup answers", async () => {
		const { source } = renderComposer();
		const lookup = deferred<GitItemSearch>();
		source.findGitItems.mockReturnValue(lookup.promise);
		const textbox = await findTextbox();
		pasteText(textbox, LINK);
		act(() => {
			editorOf(textbox).commands.insertContentAt(LINK.length, "2");
		});
		const changed = `${LINK.slice(0, -1)}2${LINK.slice(-1)}`;
		expect(textIn(textbox)).toBe(changed);

		await act(async () => lookup.resolve(own([pr])));
		await settle();
		expect(textIn(textbox)).toBe(changed);
		expect(gitPillsIn(textbox)).toEqual([]);
	});

	it("stays plain text with anything else in the paste", async () => {
		const { source } = renderComposer();
		const textbox = await findTextbox();
		const text = `see ${LINK}`;
		pasteText(textbox, text);

		await settle();
		expect(textIn(textbox)).toBe(text);
		expect(source.findGitItems).not.toHaveBeenCalled();
	});
});
