// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";
import { osOfUserAgent, wordingFor } from "./os";

describe("osOfUserAgent", () => {
	it("reads each desktop web view's user agent", () => {
		expect(
			osOfUserAgent(
				"Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)"
			)
		).toBe("macos");
		expect(
			osOfUserAgent(
				"Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0"
			)
		).toBe("windows");
		expect(
			osOfUserAgent(
				"Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15"
			)
		).toBe("linux");
	});
	it("reads anything it doesn't know as Linux", () => {
		expect(osOfUserAgent("")).toBe("linux");
		expect(osOfUserAgent("Mozilla/5.0 (X11; FreeBSD amd64)")).toBe("linux");
	});
});

describe("wordingFor", () => {
	it("keeps macOS and unknown systems on the text as written", () => {
		expect(wordingFor("macos")).toBeUndefined();
		expect(wordingFor(null)).toBeUndefined();
		expect(wordingFor(undefined)).toBeUndefined();
		expect(wordingFor("")).toBeUndefined();
	});
	it("gives Windows its own wording and every other Unix Linux's", () => {
		expect(wordingFor("windows")).toBe("windows");
		expect(wordingFor("linux")).toBe("linux");
		expect(wordingFor("freebsd")).toBe("linux");
	});
});
