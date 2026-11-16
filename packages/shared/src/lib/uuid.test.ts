// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";

import { newUuid } from "./uuid";

describe("newUuid", () => {
	it("makes a version 4 UUID, each one new", () => {
		const shape = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
		expect(newUuid()).toMatch(shape);
		expect(newUuid()).not.toBe(newUuid());
	});

	it("makes one without randomUUID too", () => {
		const real = globalThis.crypto;
		vi.stubGlobal("crypto", { getRandomValues: real.getRandomValues.bind(real) });
		try {
			expect(newUuid()).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab]/);
		} finally {
			vi.unstubAllGlobals();
		}
	});
});
