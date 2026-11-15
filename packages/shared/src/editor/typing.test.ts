// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import {
	DEFAULT_TYPING,
	readTypingPrefs,
	spellingMode,
	typingAttributes,
	writeTypingPrefs,
} from "./typing";

const IN_APP = { nativeSpelling: true, rich: true };

describe("typing settings", () => {
	it("checks spelling and corrects nothing by default", () => {
		expect(DEFAULT_TYPING).toEqual({
			checkSpelling: true,
			correctSpelling: false,
			busySend: "queue",
		});
		expect(readTypingPrefs(null)).toEqual(DEFAULT_TYPING);
	});

	it("reads back what it saved, and the defaults from anything else", () => {
		const prefs = { checkSpelling: false, correctSpelling: true, busySend: "steer" as const };
		expect(readTypingPrefs(writeTypingPrefs(prefs))).toEqual(prefs);
		expect(readTypingPrefs("{")).toEqual(DEFAULT_TYPING);
		expect(readTypingPrefs('{"version":9,"checkSpelling":false}')).toEqual(DEFAULT_TYPING);
		expect(readTypingPrefs('{"version":1,"checkSpelling":"no"}')).toEqual(DEFAULT_TYPING);
	});

	it("queues on Enter for settings saved before you could pick", () => {
		expect(
			readTypingPrefs('{"version":1,"checkSpelling":false,"correctSpelling":false}')
		).toEqual({ checkSpelling: false, correctSpelling: false, busySend: "queue" });
		expect(readTypingPrefs('{"version":1,"busySend":"interrupt"}').busySend).toBe("queue");
	});

	it("underlines through the system checker in the app, with the webview's off", () => {
		const mode = spellingMode(DEFAULT_TYPING, IN_APP);
		expect(mode).toBe("native");
		expect(typingAttributes(DEFAULT_TYPING, mode)).toEqual({
			spellcheck: "false",
			autocorrect: "off",
			autocapitalize: "off",
			writingsuggestions: "false",
		});
	});

	it("leaves a plain field unchecked in the app, since the webview's checker would correct", () => {
		expect(spellingMode(DEFAULT_TYPING, { nativeSpelling: true, rich: false })).toBe("off");
	});

	it("falls back to the webview's checker outside the app", () => {
		expect(spellingMode(DEFAULT_TYPING, { nativeSpelling: false, rich: true })).toBe("webview");
		expect(spellingMode(DEFAULT_TYPING, { nativeSpelling: false, rich: false })).toBe(
			"webview"
		);
	});

	it("hands spelling to the webview, correcting, when corrections are on", () => {
		const prefs = { ...DEFAULT_TYPING, correctSpelling: true };
		const mode = spellingMode(prefs, IN_APP);
		expect(mode).toBe("webview");
		expect(typingAttributes(prefs, mode)).toMatchObject({
			spellcheck: "true",
			autocorrect: "on",
			autocapitalize: "off",
		});
	});

	it("checks and corrects nothing with spell check off, whatever corrections say", () => {
		const prefs = { ...DEFAULT_TYPING, checkSpelling: false, correctSpelling: true };
		const mode = spellingMode(prefs, IN_APP);
		expect(mode).toBe("off");
		expect(typingAttributes(prefs, mode)).toMatchObject({
			spellcheck: "false",
			autocorrect: "off",
		});
	});
});
