// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { color, tint } from "./theme";

describe("tint", () => {
	it("turns a hex color into the same color at an opacity", () => {
		expect(tint(color.ready, 0.16)).toBe("rgba(63, 179, 127, 0.16)");
		expect(tint("#000000", 1)).toBe("rgba(0, 0, 0, 1)");
	});
});
