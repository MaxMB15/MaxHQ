// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { endedEarly, gapAbove, isOpen, placeLabel, sentFilesLine } from "./format";

describe("isOpen", () => {
	it("counts a step as open until it ends", () => {
		for (const status of ["pending", "running", "waiting"] as const) {
			expect(isOpen(status)).toBe(true);
		}
		for (const status of [
			"completed",
			"failed",
			"cancelled",
			"interrupted",
			"declined",
		] as const) {
			expect(isOpen(status)).toBe(false);
		}
	});
});

describe("endedEarly", () => {
	it("names a step that didn't finish", () => {
		expect(endedEarly("declined")).toBe("Declined");
		expect(endedEarly("cancelled")).toBe("Stopped");
		expect(endedEarly("interrupted")).toBe("Stopped");
		expect(endedEarly("failed")).toBe("Failed");
	});

	it("says nothing for any other", () => {
		expect(endedEarly("completed")).toBeNull();
		expect(endedEarly("running")).toBeNull();
	});
});

describe("gapAbove", () => {
	it("leaves no space above the first row", () => {
		expect(gapAbove("command", null)).toBe(0);
	});

	it("keeps tool steps tight together and close under reasoning", () => {
		expect(gapAbove("command", "file_read")).toBe(1);
		expect(gapAbove("search", "reasoning")).toBe(2);
	});

	it("gives your message the most room, and anything else the same", () => {
		expect(gapAbove("assistant_message", "user_message")).toBe(14);
		expect(gapAbove("command", "user_message")).toBe(14);
		expect(gapAbove("assistant_message", "command")).toBe(10);
		expect(gapAbove("user_message", "assistant_message")).toBe(10);
	});
});

describe("sentFilesLine", () => {
	it("counts the files", () => {
		expect(sentFilesLine(1)).toBe("Sent you a file");
		expect(sentFilesLine(3)).toBe("Sent you 3 files");
	});
});

describe("placeLabel", () => {
	it("counts from one", () => {
		expect(placeLabel(1, 5)).toBe("2 / 5");
	});
});
