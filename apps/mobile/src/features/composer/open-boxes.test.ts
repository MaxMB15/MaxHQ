// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it } from "vitest";

import type { AgentId } from "@maxhq/shared/bindings";

import { boxShown, openBox, openBoxes } from "./open-boxes";

const A = "agent-a" as AgentId;
const B = "agent-b" as AgentId;

afterEach(() => openBoxes.setState({ open: new Map() }));

describe("openBox", () => {
	it("shows a box until each one that opened it shuts", () => {
		const first = openBox(A);
		const second = openBox(A);
		expect(boxShown(A)).toBe(true);
		expect(boxShown(B)).toBe(false);
		first();
		expect(boxShown(A)).toBe(true);
		second();
		expect(boxShown(A)).toBe(false);
	});

	it("counts a shut once, however often it runs", () => {
		const first = openBox(A);
		openBox(A);
		first();
		first();
		expect(boxShown(A)).toBe(true);
	});
});
