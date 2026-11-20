// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { LOGIN_HINT, LOGIN_PAGES, LOGIN_STEPS, spanKind, stepParts } from "./login-steps";

describe("spanKind", () => {
	it("links a whole web address", () => {
		expect(spanKind("https://cursor.com/dashboard")).toBe("link");
	});

	it("copies a command, or a name or an address with a placeholder", () => {
		expect(spanKind("claude setup-token")).toBe("copy");
		expect(spanKind("CLAUDE_CONFIG_DIR=<dir> claude setup-token")).toBe("copy");
		expect(spanKind("https://github.com/<your GitHub login>")).toBe("copy");
		expect(spanKind("MaxHQ pushes")).toBe("copy");
	});

	it("shows anything else as plain code", () => {
		expect(spanKind("maxd")).toBe("code");
		expect(spanKind("secrets/cursor.json")).toBe("code");
	});
});

describe("stepParts", () => {
	it("splits a step at its backticks", () => {
		expect(stepParts("Run `claude setup-token` in `maxd`.")).toEqual([
			{ kind: "text", text: "Run " },
			{ kind: "copy", text: "claude setup-token" },
			{ kind: "text", text: " in " },
			{ kind: "code", text: "maxd" },
			{ kind: "text", text: "." },
		]);
	});

	it("drops the empty text around a span at either end", () => {
		expect(stepParts("`https://cursor.com/dashboard`")).toEqual([
			{ kind: "link", text: "https://cursor.com/dashboard" },
		]);
	});
});

describe("LOGIN_STEPS", () => {
	it("has a title, a hint and steps for every login", () => {
		for (const login of LOGIN_PAGES) {
			expect(LOGIN_STEPS[login].title).not.toBe("");
			expect(LOGIN_HINT[login]).not.toBe("");
			expect(LOGIN_STEPS[login].steps.length).toBeGreaterThan(0);
		}
	});

	it("closes every code span it opens", () => {
		for (const login of LOGIN_PAGES) {
			const { before, steps, after } = LOGIN_STEPS[login];
			for (const line of [...before, ...steps, ...after]) {
				expect(line.split("`").length % 2, line).toBe(1);
			}
		}
	});

	it("names no planning step", () => {
		for (const login of LOGIN_PAGES) {
			const { before, steps, after } = LOGIN_STEPS[login];
			for (const line of [...before, ...steps, ...after]) {
				expect(line, line).not.toMatch(/\b[A-Z]-\d+\b/);
			}
		}
	});
});
