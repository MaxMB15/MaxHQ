// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import {
	clampScale,
	fitScale,
	fitted,
	isFitted,
	panBy,
	pannable,
	toggleActual,
	wheelFactor,
	ZOOM_MAX,
	ZOOM_MIN,
	zoomAt,
	zoomKey,
} from "./zoom";

const FRAME = { width: 800, height: 600 };
const BIG = { width: 3200, height: 1200 };
const SMALL = { width: 200, height: 100 };

describe("fitScale", () => {
	it("shrinks a big picture to the frame's tighter side", () => {
		expect(fitScale(BIG, FRAME)).toBe(0.25);
	});

	it("never grows a small picture past its own pixels", () => {
		expect(fitScale(SMALL, FRAME)).toBe(1);
	});

	it("takes a picture with no size yet as its own pixels", () => {
		expect(fitScale({ width: 0, height: 0 }, FRAME)).toBe(1);
	});
});

describe("clampScale", () => {
	it("keeps a scale between the bounds", () => {
		expect(clampScale(20, 1)).toBe(ZOOM_MAX);
		expect(clampScale(0.01, 1)).toBe(ZOOM_MIN);
		expect(clampScale(2, 1)).toBe(2);
	});

	it("lets a fit beyond the bounds stand", () => {
		expect(clampScale(0.01, 0.05)).toBe(0.05);
	});
});

describe("fitted", () => {
	it("centres the picture on both sides", () => {
		expect(fitted(BIG, FRAME)).toEqual({ scale: 0.25, x: 0, y: 150 });
		expect(fitted(SMALL, FRAME)).toEqual({ scale: 1, x: 300, y: 250 });
	});
});

describe("zoomAt", () => {
	it("keeps the point under the pointer still", () => {
		const start = { scale: 1, x: -400, y: -300 };
		const at = { x: 200, y: 100 };
		const next = zoomAt(start, 2, at, BIG, FRAME);
		// The picture point under `at` was (600, 400); it stays under `at`.
		expect(next.scale).toBe(2);
		expect((at.x - next.x) / next.scale).toBeCloseTo(600);
		expect((at.y - next.y) / next.scale).toBeCloseTo(400);
	});

	it("stops at the largest scale", () => {
		expect(zoomAt(fitted(BIG, FRAME), 100, { x: 0, y: 0 }, BIG, FRAME).scale).toBe(ZOOM_MAX);
	});

	it("centres a side once it's smaller than the frame", () => {
		const zoomed = zoomAt(fitted(BIG, FRAME), 0.2, { x: 0, y: 0 }, BIG, FRAME);
		expect(zoomed).toEqual({ scale: 0.2, x: 80, y: 180 });
	});
});

describe("panBy", () => {
	it("moves a picture bigger than the frame", () => {
		const start = { scale: 1, x: -100, y: -100 };
		expect(panBy(start, -50, 20, BIG, FRAME)).toEqual({ scale: 1, x: -150, y: -80 });
	});

	it("never lets the picture leave the frame", () => {
		const start = { scale: 1, x: -100, y: -100 };
		expect(panBy(start, 5000, -5000, BIG, FRAME)).toEqual({ scale: 1, x: 0, y: -600 });
	});

	it("leaves a fitted picture where it is", () => {
		const start = fitted(SMALL, FRAME);
		expect(panBy(start, 40, 40, SMALL, FRAME)).toEqual(start);
	});
});

describe("pannable", () => {
	it("is set only while the picture is bigger than the frame", () => {
		expect(pannable(fitted(BIG, FRAME), BIG, FRAME)).toBe(false);
		expect(pannable({ scale: 1, x: 0, y: 0 }, BIG, FRAME)).toBe(true);
	});
});

describe("toggleActual", () => {
	it("goes from fitted to the picture's own pixels around the click", () => {
		const next = toggleActual(fitted(BIG, FRAME), { x: 400, y: 300 }, BIG, FRAME);
		expect(next.scale).toBe(1);
		expect((400 - next.x) / next.scale).toBeCloseTo(1600);
	});

	it("goes back to fitted from any other scale", () => {
		const zoomed = { scale: 3, x: -10, y: -10 };
		expect(isFitted(toggleActual(zoomed, { x: 0, y: 0 }, BIG, FRAME), BIG, FRAME)).toBe(true);
	});

	it("doubles a picture that fits at its own pixels", () => {
		expect(toggleActual(fitted(SMALL, FRAME), { x: 400, y: 300 }, SMALL, FRAME).scale).toBe(2);
	});
});

describe("zoomKey", () => {
	it("reads the keys Preview uses", () => {
		expect(zoomKey("+")).toBe("in");
		expect(zoomKey("=")).toBe("in");
		expect(zoomKey("-")).toBe("out");
		expect(zoomKey("0")).toBe("fit");
		expect(zoomKey("1")).toBe("actual");
		expect(zoomKey("ArrowLeft")).toBeNull();
	});
});

describe("wheelFactor", () => {
	it("zooms in on a pinch out and out on a pinch in", () => {
		expect(wheelFactor(-10, false)).toBeGreaterThan(1);
		expect(wheelFactor(10, false)).toBeLessThan(1);
	});

	it("limits one event's step, a mouse wheel's lines included", () => {
		expect(wheelFactor(-4, true)).toBeCloseTo(Math.exp(0.5));
		expect(wheelFactor(-1000, false)).toBeCloseTo(Math.exp(0.5));
	});
});
