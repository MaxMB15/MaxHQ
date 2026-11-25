// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ariaKeys, keyHint, plainHint, type Press, SHORTCUTS, throughProgram } from "./shortcuts";

const MAC = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)";
const WINDOWS = "Mozilla/5.0 (Windows NT 10.0; Win64; x64)";

afterEach(() => {
	vi.restoreAllMocks();
});

describe("how a shortcut is written", () => {
	it("uses the Mac's symbols on a Mac", () => {
		vi.spyOn(navigator, "userAgent", "get").mockReturnValue(MAC);
		expect(keyHint(SHORTCUTS.splitRight)).toBe("⌘D");
		expect(keyHint(SHORTCUTS.splitDown)).toBe("⇧⌘D");
		expect(keyHint({ label: "←", code: "ArrowLeft", alt: true })).toBe("⌥⌘←");
		expect(ariaKeys(SHORTCUTS.reopen)).toBe("Meta+Shift+T");
		// The terminal's holds Control on a Mac too.
		expect(keyHint(SHORTCUTS.terminal)).toBe("⌃`");
		expect(ariaKeys(SHORTCUTS.terminal)).toBe("Control+`");
		expect(keyHint(SHORTCUTS.settings)).toBe("⌘,");
		expect(ariaKeys(SHORTCUTS.settings)).toBe("Meta+,");
		expect(plainHint({ mac: "↩", other: "Enter" }, { alt: true })).toBe("⌥↩");
		expect(plainHint("C", { control: true })).toBe("⌃C");
		expect(plainHint({ mac: "⏎", other: "Enter" }, { shift: true })).toBe("⇧⏎");
	});

	it("spells the keys out elsewhere", () => {
		vi.spyOn(navigator, "userAgent", "get").mockReturnValue(WINDOWS);
		expect(keyHint(SHORTCUTS.splitDown)).toBe("Ctrl+Shift+D");
		expect(keyHint(SHORTCUTS.nextWaiting)).toBe("Ctrl+J");
		expect(ariaKeys(SHORTCUTS.close)).toBe("Control+Shift+W");
		expect(keyHint(SHORTCUTS.terminal)).toBe("Ctrl+`");
		expect(ariaKeys(SHORTCUTS.settings)).toBe("Control+,");
		expect(plainHint({ mac: "↩", other: "Enter" }, { alt: true })).toBe("Alt+Enter");
		expect(plainHint("C", { control: true })).toBe("Ctrl+C");
	});
});

describe("a shortcut past a program that keeps Control", () => {
	const press = (init: Partial<Press>): Press => ({
		key: "J",
		code: "KeyJ",
		ctrlKey: true,
		metaKey: false,
		altKey: false,
		shiftKey: true,
		repeat: false,
		...init,
	});
	let card: HTMLElement;
	let outside: HTMLElement;

	beforeEach(() => {
		document.body.innerHTML =
			'<div data-keys="held"><span id="card">x</span></div><span id="outside">y</span>';
		card = document.getElementById("card")!;
		outside = document.getElementById("outside")!;
	});

	it("reads Control and Shift as Control in a card holding your keys, off a Mac", () => {
		vi.spyOn(navigator, "userAgent", "get").mockReturnValue(WINDOWS);
		expect(throughProgram(press({}), card)).toEqual(press({ shiftKey: false }));
		expect(throughProgram(press({ altKey: true }), card)).toEqual(
			press({ altKey: true, shiftKey: false })
		);
	});

	it("has no other reading anywhere else, without Shift, or on a Mac", () => {
		vi.spyOn(navigator, "userAgent", "get").mockReturnValue(WINDOWS);
		expect(throughProgram(press({}), outside)).toBeNull();
		expect(throughProgram(press({}), null)).toBeNull();
		expect(throughProgram(press({ shiftKey: false }), card)).toBeNull();
		expect(throughProgram(press({ metaKey: true }), card)).toBeNull();
		vi.spyOn(navigator, "userAgent", "get").mockReturnValue(MAC);
		expect(throughProgram(press({ ctrlKey: false, metaKey: true }), card)).toBeNull();
		expect(throughProgram(press({}), card)).toBeNull();
	});
});
