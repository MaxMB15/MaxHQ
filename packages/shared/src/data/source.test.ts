// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { pickSource } from "./source";

describe("pickSource", () => {
	it("follows the switch when it's set", () => {
		expect(pickSource("tauri", true, true)).toBe("tauri");
		expect(pickSource("mock", false, true)).toBe("mock");
	});

	it("uses sample data in dev and outside the app", () => {
		expect(pickSource(undefined, true, true)).toBe("mock");
		expect(pickSource(undefined, false, false)).toBe("mock");
		expect(pickSource("", false, false)).toBe("mock");
	});

	it("uses the backend in a build inside the app", () => {
		expect(pickSource(undefined, false, true)).toBe("tauri");
	});
});
