// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it, vi } from "vitest";

import { ariaKeys, keyHint, SHORTCUTS } from "./shortcuts";

afterEach(() => {
	vi.restoreAllMocks();
});

describe("how a shortcut is written", () => {
	it("uses the Mac's symbols on a Mac", () => {
		vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
			"Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)"
		);
		expect(keyHint(SHORTCUTS.splitRight)).toBe("⌘D");
		expect(keyHint(SHORTCUTS.splitDown)).toBe("⇧⌘D");
		expect(keyHint({ label: "←", code: "ArrowLeft", alt: true })).toBe("⌥⌘←");
		expect(ariaKeys(SHORTCUTS.reopen)).toBe("Meta+Shift+T");
		// The terminal's holds Control on a Mac too.
		expect(keyHint(SHORTCUTS.terminal)).toBe("⌃`");
		expect(ariaKeys(SHORTCUTS.terminal)).toBe("Control+`");
	});

	it("spells the keys out elsewhere", () => {
		vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
			"Mozilla/5.0 (Windows NT 10.0; Win64; x64)"
		);
		expect(keyHint(SHORTCUTS.splitDown)).toBe("Ctrl+Shift+D");
		expect(keyHint(SHORTCUTS.nextWaiting)).toBe("Ctrl+J");
		expect(ariaKeys(SHORTCUTS.close)).toBe("Control+Shift+W");
		expect(keyHint(SHORTCUTS.terminal)).toBe("Ctrl+`");
	});
});
