// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { ActionResult } from "../data/source";

import { readOf } from "./read";

describe("readOf", () => {
	it("gives the outcome asked for", () => {
		const result: ActionResult = { status: "ok", data: { status: "pulls", pulls: [] } };
		expect(readOf(result, "pulls").pulls).toEqual([]);
	});

	it("throws a refusal's reason", () => {
		const result: ActionResult = {
			status: "ok",
			data: { status: "refused", reason: "No App." },
		};
		expect(() => readOf(result, "pulls")).toThrow("No App.");
	});

	it("throws when the device can't be reached", () => {
		const result: ActionResult = {
			status: "error",
			error: { kind: "not_connected", device: "devbox" },
		};
		expect(() => readOf(result, "pull")).toThrow("devbox isn't connected.");
	});

	it("throws on an answer it didn't ask for", () => {
		const result: ActionResult = { status: "ok", data: { status: "done" } };
		expect(() => readOf(result, "pull")).toThrow("The device answered done.");
	});
});
