// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { searchWords } from "@maxhq/shared/lib/words";

import { excerpt, findIn, highlight } from "./search";

const TASKS = {
	title: "Scheduled tasks",
	terms: ["Prompts each device sends on a timer", "Webhook", "Rotate URL"],
};

describe("settings search", () => {
	it("marks each place a word is found", () => {
		expect(highlight("Rotate URL", ["url"])).toEqual([
			{ text: "Rotate ", hit: false },
			{ text: "URL", hit: true },
		]);
		expect(highlight("Café menu", ["cafe", "nu"])).toEqual([
			{ text: "Café", hit: true },
			{ text: " me", hit: false },
			{ text: "nu", hit: true },
		]);
		expect(highlight("Keys", ["x"])).toEqual([{ text: "Keys", hit: false }]);
	});

	it("matches every page and marks nothing when there are no words", () => {
		expect(findIn(TASKS, [])).toEqual({
			title: [{ text: "Scheduled tasks", hit: false }],
			term: null,
		});
	});

	it("shows only the title when the title has every word", () => {
		expect(findIn(TASKS, searchWords("sched"))).toEqual({
			title: [
				{ text: "Sched", hit: true },
				{ text: "uled tasks", hit: false },
			],
			term: null,
		});
	});

	it("shows the heading or label that has the words the title lacks", () => {
		expect(findIn(TASKS, searchWords("tasks webhook"))).toEqual({
			title: [
				{ text: "Scheduled ", hit: false },
				{ text: "tasks", hit: true },
			],
			term: [{ text: "Webhook", hit: true }],
		});
		expect(findIn(TASKS, searchWords("rotate url"))?.term).toEqual([
			{ text: "Rotate", hit: true },
			{ text: " ", hit: false },
			{ text: "URL", hit: true },
		]);
	});

	it("picks the shortest text that has the words, since a label says more", () => {
		const tools = {
			title: "Agent tools",
			terms: ["For tools outside MaxHQ, such as a bot", "Access keys", "Key name"],
		};
		expect(findIn(tools, searchWords("key"))?.term).toEqual([
			{ text: "Key", hit: true },
			{ text: " name", hit: false },
		]);
	});

	it("starts a long text at the match when the list would cut the match off", () => {
		const hint = "Prompts each device sends to an agent on a timer, a webhook or GitHub events";
		expect(excerpt(hint, ["github"])).toBe("…GitHub events");
		expect(excerpt(hint, ["timer", "prompts"])).toBe(hint);
		expect(excerpt(hint, ["events", "webhook"])).toBe("…webhook or GitHub events");
		expect(excerpt("Webhook", ["webhook"])).toBe("Webhook");
		expect(findIn({ title: "Scheduled tasks", terms: [hint] }, ["github"])?.term).toEqual([
			{ text: "…", hit: false },
			{ text: "GitHub", hit: true },
			{ text: " events", hit: false },
		]);
	});

	it("leaves a page out unless every word is on it", () => {
		expect(findIn(TASKS, searchWords("webhook phone"))).toBeNull();
		expect(findIn(TASKS, searchWords("pairing"))).toBeNull();
	});
});
