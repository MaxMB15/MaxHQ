// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { LARGEST, MOST_ZOOM, wholeLayout } from "./whole";

const PHONE = { width: 390, height: 700 };

describe("wholeLayout", () => {
	it("lays a large drawing out at its own size and starts zoomed out to fit", () => {
		const layout = wholeLayout({ width: 1560, height: 700 }, PHONE);
		expect(layout).toMatchObject({ width: 1560, height: 700, least: 0.25 });
		expect(layout.most).toBe(MOST_ZOOM * 0.25);
	});

	it("lets a pinch reach the drawing's own size, however far that is", () => {
		const layout = wholeLayout({ width: 2000, height: 400 }, PHONE);
		expect(layout.least).toBeCloseTo(0.195);
		expect(layout.most).toBe(1);
	});

	it("lays a small drawing out to fit the screen", () => {
		expect(wholeLayout({ width: 195, height: 100 }, PHONE)).toEqual({
			width: 390,
			height: 200,
			least: 1,
			most: MOST_ZOOM,
		});
	});

	it("lays a very large drawing out no longer than the largest side", () => {
		const layout = wholeLayout({ width: 1000, height: 8192 }, PHONE);
		expect(layout).toMatchObject({ width: 250, height: LARGEST });
		expect(layout.least * LARGEST).toBeCloseTo(700);
	});
});
