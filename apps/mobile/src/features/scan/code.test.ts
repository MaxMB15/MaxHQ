// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { afterScan, NOT_A_CODE, pairCodeOf, scanLinkOf } from "./code";

describe("pairCodeOf", () => {
	it("takes the Mac's code, trimmed", () => {
		expect(pairCodeOf("  maxhq:AQID\n")).toBe("maxhq:AQID");
	});

	it("passes over anything else", () => {
		expect(pairCodeOf("https://example.com")).toBeNull();
		expect(pairCodeOf("maxhq:")).toBeNull();
		expect(pairCodeOf("")).toBeNull();
	});
});

describe("afterScan", () => {
	it("stops at a confirmation for a code", () => {
		expect(afterScan(" maxhq:AQID\n")).toEqual({ confirm: "maxhq:AQID" });
	});

	it("keeps scanning, with a hint, past anything else", () => {
		expect(afterScan("https://example.com")).toEqual({ hint: NOT_A_CODE });
	});
});

describe("scanLinkOf", () => {
	it("sends a code opened as a link to the scan screen", () => {
		expect(scanLinkOf("maxhq:AQID-_x")).toBe("maxhq://scan?code=maxhq%3AAQID-_x");
	});

	it("leaves the app's own links and anything else", () => {
		expect(scanLinkOf("maxhq://agent/a1")).toBe("maxhq://agent/a1");
		expect(scanLinkOf("maxhq://scan")).toBe("maxhq://scan");
		expect(scanLinkOf("maxhq:")).toBe("maxhq:");
		expect(scanLinkOf("https://example.com")).toBe("https://example.com");
	});
});
