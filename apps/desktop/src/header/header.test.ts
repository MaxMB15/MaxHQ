// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { bellLabel } from "./notifications";

// The usage popover's wording is tested with it in
// packages/shared/src/status/usage-text.test.ts.

describe("header", () => {
	it("says how many agents need you on the bell", () => {
		expect(bellLabel(0)).toBe("No agent needs you");
		expect(bellLabel(1)).toBe("1 agent needs you");
		expect(bellLabel(3)).toBe("3 agents need you");
		// With Show the badge off the bell has no count to say.
		expect(bellLabel(null)).toBe("Notifications");
	});
});
