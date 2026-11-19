// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { color } from "../../theme";
import { LOOK_TEXT, madeShare, STEP_DOT } from "./look";

describe("workspace looks", () => {
	it("colors a state's words as its dot", () => {
		expect(LOOK_TEXT.live).toBe(color.readyText);
		expect(LOOK_TEXT.unreachable).toBe(color.errorText);
	});

	it("rings only the steps not reached yet", () => {
		expect(Object.entries(STEP_DOT).filter(([, dot]) => dot.dashed)).toEqual([
			["todo", { color: color.off, dashed: true }],
		]);
	});

	it("fills the bar by the steps done, never past either end", () => {
		expect(madeShare(2, 4)).toBe(0.5);
		expect(madeShare(0, 0)).toBe(0);
		expect(madeShare(5, 4)).toBe(1);
	});
});
