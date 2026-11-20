// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { findQuote } from "@maxhq/shared/pills/anchor";
import { describe, expect, it } from "vitest";

import { quoteOfSelection } from "./quote";

describe("quoteOfSelection", () => {
	it("starts where the words show up, counting whitespace runs as one space", () => {
		const text = "First line.\n\nThe cache   misses on the second poll.";
		const anchor = quoteOfSelection(text, " cache misses \n");
		expect(anchor).toEqual({ text: "cache misses", position: 16 });
		expect(findQuote(text, anchor!)).toEqual({ from: 17, to: 31 });
	});

	it("starts at the beginning when formatting hides the words", () => {
		expect(quoteOfSelection("**fix** it", "fix it")).toEqual({ text: "fix it", position: 0 });
	});

	it("takes nothing from a blank selection", () => {
		expect(quoteOfSelection("text", " \n ")).toBeNull();
	});
});
