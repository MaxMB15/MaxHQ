// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import type { Services } from "@/app/services";
import type { Question } from "@/components/confirm";
import { events } from "@/data/tauri-commands";
import { pageLinkQuestion, visualLinkQuestion } from "@maxhq/shared/visuals/links";

import { askToOpen, CLICK_MS, linkClicks, openRefused, watchPageLinks } from "./page-links";

vi.mock("./platform", () => ({ inApp: () => true }));

// The window's question, as the tests answer it.
const dialog = vi.hoisted(() => ({
	asking: false,
	answer: true,
	asked: [] as Question[],
}));

vi.mock("@/components/confirm", () => ({
	asking: () => dialog.asking,
	requestConfirm: async (_services: Services, question: Question) => {
		dialog.asked.push(question);
		return dialog.answer;
	},
}));

const services = {} as Services;
let now = Date.UTC(2026, 10, 21);

beforeAll(() => {
	vi.useFakeTimers({ toFake: ["Date"] });
});
afterAll(() => {
	vi.useRealTimers();
});
beforeEach(() => {
	// Each test starts well after the last question was answered.
	now += 60_000;
	vi.setSystemTime(now);
	dialog.asking = false;
	dialog.answer = true;
	dialog.asked = [];
});
afterEach(() => {
	vi.restoreAllMocks();
});

const opens = () => vi.spyOn(window, "open").mockReturnValue(null);

describe("the link you clicked last", () => {
	it("matches the app's word about it once", () => {
		const clicks = linkClicks();
		clicks.note("https://example.com", 1000);
		expect(clicks.take("https://example.com/", 1500)).toBe(true);
		expect(clicks.take("https://example.com/", 1600)).toBe(false);
	});

	it("matches only that address, and only soon after", () => {
		const clicks = linkClicks();
		clicks.note("https://example.com/", 1000);
		expect(clicks.take("https://example.org/", 1100)).toBe(false);
		expect(clicks.take("https://example.com/", 1000 + CLICK_MS + 1)).toBe(false);
	});
});

describe("an address the window stayed away from", () => {
	it("opens at once after a click on it", () => {
		const open = opens();
		const clicks = linkClicks();
		clicks.note("https://example.com/docs", Date.now());
		openRefused(services, clicks, "https://example.com/docs");
		expect(open).toHaveBeenCalledWith("https://example.com/docs", "_blank", "noreferrer");
		expect(dialog.asked).toEqual([]);
	});

	it("asks first without one, showing the whole address", async () => {
		const open = opens();
		openRefused(services, linkClicks(), "https://example.com/?q=notes");
		await vi.waitFor(() => expect(open).toHaveBeenCalled());
		expect(dialog.asked).toEqual([
			expect.objectContaining({
				...pageLinkQuestion(),
				link: "https://example.com/?q=notes",
			}),
		]);
	});
});

describe("asking to open a link", () => {
	it("opens it on Open and not on Cancel", async () => {
		const open = opens();
		dialog.answer = false;
		await askToOpen(services, "https://example.com/", visualLinkQuestion("Claude Code"));
		expect(dialog.asked).toHaveLength(1);
		expect(open).not.toHaveBeenCalled();
		vi.setSystemTime((now += 2000));
		dialog.answer = true;
		await askToOpen(services, "https://example.com/", visualLinkQuestion("Claude Code"));
		expect(open).toHaveBeenCalledWith("https://example.com/", "_blank", "noreferrer");
	});

	it("asks nothing while another question waits, or for an address that isn't web or mail", async () => {
		dialog.asking = true;
		await askToOpen(services, "https://example.com/", visualLinkQuestion(null));
		dialog.asking = false;
		await askToOpen(services, "file:///etc/hosts", visualLinkQuestion(null));
		await askToOpen(services, "javascript:alert(1)", visualLinkQuestion(null));
		expect(dialog.asked).toEqual([]);
	});

	it("asks one at a time, and not again within a second of the answer", async () => {
		dialog.answer = false;
		const first = askToOpen(services, "https://example.com/1", visualLinkQuestion(null));
		void askToOpen(services, "https://example.com/2", visualLinkQuestion(null));
		await first;
		await askToOpen(services, "https://example.com/3", visualLinkQuestion(null));
		expect(dialog.asked.map((question) => question.link)).toEqual(["https://example.com/1"]);
		vi.setSystemTime((now += 1000));
		await askToOpen(services, "https://example.com/4", visualLinkQuestion(null));
		expect(dialog.asked).toHaveLength(2);
	});
});

describe("watching the window's links", () => {
	it("asks about an address the window stayed away from, and stops quietly", async () => {
		let heard: Parameters<typeof events.openAsked.listen>[0] | undefined;
		// A window going away may have lost its events already.
		const unlisten = vi.fn(() => Promise.reject(new Error("The window lost its events.")));
		vi.spyOn(events.openAsked, "listen").mockImplementation(async (onEvent) => {
			heard = onEvent;
			return unlisten;
		});
		const open = opens();
		const stop = watchPageLinks(services);
		await vi.waitFor(() => expect(heard).toBeDefined());
		heard?.({ event: "open-asked", id: 1, payload: { url: "https://example.com/" } });
		await vi.waitFor(() => expect(open).toHaveBeenCalled());
		expect(dialog.asked).toEqual([expect.objectContaining({ link: "https://example.com/" })]);
		stop();
		await vi.waitFor(() => expect(unlisten).toHaveBeenCalled());
	});

	it("stops listening when it stopped before the listening began", async () => {
		const unlisten = vi.fn();
		vi.spyOn(events.openAsked, "listen").mockResolvedValue(unlisten);
		watchPageLinks(services)();
		await vi.waitFor(() => expect(unlisten).toHaveBeenCalled());
	});
});
