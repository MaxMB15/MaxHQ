// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { ACTIVITY_LABEL } from "./activity";
import { CONNECTION_LABEL } from "./connection";
import { ACTIVITY_DOT, CONNECTION_DOT, type DotShape } from "./dot";

const key = (shape: DotShape) => JSON.stringify(shape);

describe("dot shapes", () => {
	it("draws every activity look, each its own way", () => {
		expect(Object.keys(ACTIVITY_DOT).sort()).toEqual(Object.keys(ACTIVITY_LABEL).sort());
		const shapes = Object.values(ACTIVITY_DOT).map(key);
		expect(new Set(shapes).size).toBe(shapes.length);
	});

	it("draws every connection look, each its own way", () => {
		expect(Object.keys(CONNECTION_DOT).sort()).toEqual(Object.keys(CONNECTION_LABEL).sort());
		const shapes = Object.values(CONNECTION_DOT).map(key);
		expect(new Set(shapes).size).toBe(shapes.length);
	});

	it("tells starting from busy by speed, not only by color", () => {
		expect(ACTIVITY_DOT.starting).toMatchObject({ kind: "spin", slow: true });
		expect(ACTIVITY_DOT.busy).toMatchObject({ kind: "spin", slow: false });
	});

	it("gives the states that share a color different shapes", () => {
		const ready = Object.values(ACTIVITY_DOT).filter((s) => s.tone === "ready");
		expect(new Set(ready.map((s) => s.kind)).size).toBe(ready.length);
	});

	it("draws done with work still going on apart from done and from busy", () => {
		expect(ACTIVITY_DOT.idle_running.kind).toBe("orbit");
		expect(ACTIVITY_DOT.idle_waiting.kind).toBe("orbit");
		for (const look of ["idle", "done_unread", "busy", "needs_input"] as const) {
			expect(ACTIVITY_DOT[look].kind).not.toBe("orbit");
		}
	});
});
