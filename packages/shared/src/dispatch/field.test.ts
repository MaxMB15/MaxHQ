// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { enterAction, FIELD_LINES, fieldMaxHeight } from "./field";

describe("the dispatch field", () => {
	it("sends a draft, confirms a plan when blank, and does nothing otherwise", () => {
		expect(enterAction("Check the reviewers", true)).toBe("send");
		expect(enterAction("Check\nthe reviewers", false)).toBe("send");
		expect(enterAction("  \n ", true)).toBe("confirm");
		expect(enterAction("", false)).toBe("none");
	});

	it("grows to its lines and padding", () => {
		expect(fieldMaxHeight(FIELD_LINES.desktop, 20, 7)).toBe(174);
		expect(fieldMaxHeight(FIELD_LINES.phone, 20, 8)).toBe(116);
	});
});
