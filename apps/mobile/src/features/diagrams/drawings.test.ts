// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { createDrawings, type Drawing, drawingKey } from "./drawings";

const drawn: Drawing = { state: "drawn", body: "<img>", width: 10, height: 5 };

describe("a drawing's key", () => {
	it("tells kinds, sizes and texts apart", () => {
		const key = drawingKey("diagram", "graph TD; a-->b");
		expect(drawingKey("diagram", "graph TD; a-->b")).toBe(key);
		expect(drawingKey("math", "graph TD; a-->b")).not.toBe(key);
		expect(drawingKey("diagram", "graph TD; a-->c")).not.toBe(key);
		expect(drawingKey("math", "x^2", 17)).not.toBe(drawingKey("math", "x^2", 20));
	});
});

describe("the drawings", () => {
	it("draw each thing asked for once, one at a time for each page", () => {
		const { store, ask, take, settle } = createDrawings();
		ask("diagram", "a");
		ask("diagram", "a");
		ask("math", "x", 17);
		ask("diagram", "b");
		expect(store.getState().pages).toEqual(["diagram", "math"]);
		expect(store.getState().waiting).toHaveLength(3);

		const first = take("diagram");
		expect(first?.source).toBe("a");
		expect(take("diagram")).toBeNull();
		ask("diagram", "a");
		expect(store.getState().waiting).toHaveLength(2);

		const formula = take("math");
		expect(formula).toMatchObject({ kind: "math", source: "x", size: 17 });

		settle("diagram", first!.key, drawn);
		expect(store.getState().drawn.get(first!.key)).toBe(drawn);
		ask("diagram", "a");
		expect(store.getState().waiting.map((job) => job.source)).toEqual(["b"]);
		expect(take("diagram")?.source).toBe("b");
		expect(take("diagram")).toBeNull();
	});

	it("drop an answer for what the page isn't drawing", () => {
		const { store, ask, take, settle } = createDrawings();
		ask("diagram", "a");
		const job = take("diagram")!;
		settle("diagram", "diagram:0:1:1", drawn);
		expect(store.getState().drawing.diagram).toBe(job);
		expect(store.getState().drawn.size).toBe(0);
	});

	it("give up on what a page is drawing, and keep why", () => {
		const { store, ask, take, giveUp } = createDrawings();
		giveUp("math", "nothing to give up");
		expect(store.getState().drawn.size).toBe(0);
		ask("math", "x", 17);
		const job = take("math")!;
		giveUp("math", "It took too long to draw.");
		expect(store.getState().drawing.math).toBeNull();
		expect(store.getState().drawn.get(job.key)).toEqual({
			state: "failed",
			reason: "It took too long to draw.",
		});
	});

	it("keep the latest two hundred", () => {
		const { store, ask, take, settle } = createDrawings();
		const keys: string[] = [];
		for (let i = 0; i < 201; i++) {
			ask("diagram", `graph ${i}`);
			const job = take("diagram")!;
			keys.push(job.key);
			settle("diagram", job.key, drawn);
		}
		const { drawn: kept } = store.getState();
		expect(kept.size).toBe(200);
		expect(kept.has(keys[0]!)).toBe(false);
		expect(kept.has(keys[200]!)).toBe(true);
	});
});
