// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { themeChoiceOf, themeFor } from "./theme";

describe("the theme choice", () => {
	it("reads each of the three choices back", () => {
		expect(themeChoiceOf("system")).toBe("system");
		expect(themeChoiceOf("light")).toBe("light");
		expect(themeChoiceOf("dark")).toBe("dark");
	});

	it("reads anything else as System", () => {
		for (const stored of [null, undefined, "", "Dark", "sepia", 1, {}]) {
			expect(themeChoiceOf(stored)).toBe("system");
		}
	});

	it("follows the computer's appearance on System", () => {
		expect(themeFor("system", true)).toBe("dark");
		expect(themeFor("system", false)).toBe("light");
	});

	it("keeps Light and Dark whatever the computer shows", () => {
		expect(themeFor("light", true)).toBe("light");
		expect(themeFor("dark", false)).toBe("dark");
	});
});
