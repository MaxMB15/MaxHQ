// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import type { InvokeArgs } from "@tauri-apps/api/core";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { setLanguageChoice } from "@maxhq/shared/i18n";
import { NativeFailure } from "./failure";
import { checkSpelling, hasNativeSpelling, spellingMenu, type Misspelling } from "./spelling";

const app = globalThis as { isTauri?: boolean };

const TOMMOROW: Misspelling = { start: 11, end: 19, word: "tommorow", language: "en" };

describe("in the app", () => {
	const calls: [string, InvokeArgs | undefined][] = [];

	beforeEach(() => {
		app.isTauri = true;
		calls.length = 0;
		mockIPC((command, args) => {
			calls.push([command, args]);
			switch (command) {
				case "check_spelling":
					return [TOMMOROW];
				case "spelling_menu":
					if (args && "word" in args && args.word === null) {
						return Promise.reject({ kind: "unsupported", what: "the spelling menu" });
					}
					return { kind: "replace", word: "tomorrow" };
			}
		});
	});

	afterEach(() => {
		delete app.isTauri;
		clearMocks();
		vi.restoreAllMocks();
	});

	it("follows macOS only on a Mac", () => {
		const agent = vi.spyOn(navigator, "userAgent", "get");
		agent.mockReturnValue(
			"Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15"
		);
		expect(hasNativeSpelling()).toBe(true);
		agent.mockReturnValue("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Edg/140.0");
		expect(hasNativeSpelling()).toBe(false);
	});

	it("checks the text through the backend", async () => {
		await expect(checkSpelling("Ship it 🚀 tommorow.")).resolves.toEqual([TOMMOROW]);
		expect(calls).toEqual([["check_spelling", { text: "Ship it 🚀 tommorow." }]]);
	});

	it("opens the menu for a word in its language", async () => {
		await expect(spellingMenu(TOMMOROW, { x: 120, y: 64 })).resolves.toEqual({
			kind: "replace",
			word: "tomorrow",
		});
		expect(calls).toEqual([
			[
				"spelling_menu",
				{
					word: "tommorow",
					language: "en",
					at: { x: 120, y: 64 },
					labels: {
						no_guesses: "No Guesses Found",
						ignore: "Ignore Spelling",
						learn: "Learn Spelling",
						cut: "Cut",
						copy: "Copy",
						paste: "Paste",
					},
				},
			],
		]);
	});

	it("localizes the menu while keeping the word's spelling language", async () => {
		setLanguageChoice("nl");
		try {
			await spellingMenu(TOMMOROW, { x: 0, y: 0 });
			expect(calls.at(-1)?.[1]).toMatchObject({
				language: "en",
				labels: { copy: "Kopiëren", paste: "Plakken" },
			});
		} finally {
			setLanguageChoice("en");
		}
	});

	it("turns a backend error into a NativeFailure", async () => {
		const failure = await spellingMenu(null, { x: 0, y: 0 }).catch((error: unknown) => error);
		expect(failure).toBeInstanceOf(NativeFailure);
		expect((failure as NativeFailure).message).toBe(
			"the spelling menu isn't available on this platform"
		);
	});
});

describe("in a browser", () => {
	it("finds nothing and shows no menu", async () => {
		expect(hasNativeSpelling()).toBe(false);
		await expect(checkSpelling("tommorow")).resolves.toEqual([]);
		await expect(spellingMenu(TOMMOROW, { x: 0, y: 0 })).resolves.toEqual({ kind: "none" });
	});
});
