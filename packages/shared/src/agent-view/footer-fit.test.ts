// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { footerSteps } from "./footer-fit";

const everything = { sized: true, attachments: true, effort: true, permission: true };

describe("footer folds", () => {
	it("folds the attachments first, then the key hints, the pickers and the send button", () => {
		expect(footerSteps(everything)).toEqual([
			"size",
			"count",
			"keys",
			"permission",
			"effort",
			"model",
			"send",
		]);
	});

	it("leaves out the steps with nothing to fold", () => {
		expect(
			footerSteps({ sized: false, attachments: false, effort: false, permission: true })
		).toEqual(["keys", "permission", "model", "send"]);
		expect(footerSteps({ ...everything, sized: false })).not.toContain("size");
	});
});
