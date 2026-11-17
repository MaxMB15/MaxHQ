// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { lastByte, stepFor } from "./stream";

const bytes = (text: string) => new TextEncoder().encode(text);
const text = (step: ReturnType<typeof stepFor>) =>
	step ? { ...step, bytes: new TextDecoder().decode(step.bytes) } : null;

describe("stepFor", () => {
	it("writes the first output and moves the end past it", () => {
		expect(text(stepFor(null, { type: "output", offset: 4096, bytes: bytes("ok") }))).toEqual({
			clear: false,
			bytes: "ok",
			end: 4098,
		});
	});

	it("skips what the view already shows and writes the rest", () => {
		expect(stepFor(10, { type: "output", offset: 4, bytes: bytes("abcdef") })).toBeNull();
		expect(text(stepFor(10, { type: "output", offset: 8, bytes: bytes("abcdef") }))).toEqual({
			clear: false,
			bytes: "cdef",
			end: 14,
		});
	});

	it("clears and goes on after a gap", () => {
		expect(text(stepFor(10, { type: "output", offset: 20, bytes: bytes("x") }))).toEqual({
			clear: true,
			bytes: "x",
			end: 21,
		});
	});

	it("clears on a reset past its end and ignores one it already covers", () => {
		expect(stepFor(10, { type: "reset", from: 8 })).toBeNull();
		expect(text(stepFor(10, { type: "reset", from: 64 }))).toEqual({
			clear: true,
			bytes: "",
			end: 64,
		});
		expect(text(stepFor(null, { type: "reset", from: 64 }))).toEqual({
			clear: false,
			bytes: "",
			end: 64,
		});
	});
});

describe("lastByte", () => {
	it("names the last byte a view shows, and nothing before the first", () => {
		expect(lastByte(null)).toBeNull();
		expect(lastByte(0)).toBeNull();
		expect(lastByte(5)).toBe(4);
	});
});
