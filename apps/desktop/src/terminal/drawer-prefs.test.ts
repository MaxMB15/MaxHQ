// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it } from "vitest";

import type { Services } from "@/app/services";

import {
	DEFAULT_HEIGHT,
	DRAWERS_KEY,
	drawerOf,
	drawersOf,
	MAX_HEIGHT,
	MAX_SAVED,
	MIN_HEIGHT,
	readDrawers,
	setActive,
	setDrawer,
	setSplit,
} from "./drawer-prefs";

// The drawers only key their store by the services.
const services = () => ({}) as Services;

const saved = (drawers: unknown, version = 1) => JSON.stringify({ version, drawers });

afterEach(() => {
	localStorage.clear();
});

describe("reading saved drawers", () => {
	it("falls back to none for nothing, junk or another version", () => {
		expect(readDrawers(null)).toEqual({});
		expect(readDrawers("{not json")).toEqual({});
		expect(readDrawers(saved({ a: { open: true, height: 300 } }, 2))).toEqual({});
		expect(readDrawers(JSON.stringify([1, 2]))).toEqual({});
	});

	it("keeps each drawer that checks out, holding its height in bounds", () => {
		const drawers = readDrawers(
			saved({
				a: { open: true, height: 300.4 },
				b: { open: "yes", height: 5 },
				c: { height: 99_999 },
				d: "junk",
				e: { open: false, height: Number.NaN },
			})
		);
		expect(drawers).toEqual({
			a: { open: true, height: 300 },
			b: { open: false, height: MIN_HEIGHT },
			c: { open: false, height: MAX_HEIGHT },
			e: { open: false, height: DEFAULT_HEIGHT },
		});
	});

	it("keeps only the drawers used last", () => {
		const many = Object.fromEntries(
			Array.from({ length: MAX_SAVED + 5 }, (_, i) => [
				`agent-${i}`,
				{ open: true, height: 200 },
			])
		);
		const drawers = Object.keys(readDrawers(saved(many)));
		expect(drawers).toHaveLength(MAX_SAVED);
		expect(drawers[0]).toBe("agent-5");
	});
});

describe("changing a drawer", () => {
	it("saves it last, clamped, and reads the defaults for one never opened", () => {
		const window = services();
		const store = drawersOf(window);
		expect(drawerOf(store.getState(), "a")).toEqual({ open: false, height: DEFAULT_HEIGHT });
		setDrawer(window, "a", { open: true });
		setDrawer(window, "b", { height: 10 });
		setDrawer(window, "a", { height: 420 });
		expect(Object.keys(store.getState().saved)).toEqual(["b", "a"]);
		expect(readDrawers(localStorage.getItem(DRAWERS_KEY))).toEqual({
			b: { open: false, height: MIN_HEIGHT },
			a: { open: true, height: 420 },
		});
		// A new window reads what the last one saved.
		expect(drawerOf(drawersOf(services()).getState(), "a")).toEqual({
			open: true,
			height: 420,
		});
	});

	it("swaps the split tabs when you pick the one beside, so both stay in sight", () => {
		const window = services();
		const store = drawersOf(window);
		setActive(window, "a", "t-1");
		setSplit(window, "a", "t-2");
		setActive(window, "a", "t-2");
		expect(store.getState()).toMatchObject({ active: { a: "t-2" }, split: { a: "t-1" } });
		setActive(window, "a", "t-3");
		expect(store.getState()).toMatchObject({ active: { a: "t-3" }, split: { a: "t-1" } });
		setSplit(window, "a", null);
		expect(store.getState().split).toEqual({});
	});
});
