// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import type { SerializedDockview } from "dockview-react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { layoutKey, loadLayout, saveLayout } from "./storage";

const LAYOUT: SerializedDockview = {
	grid: {
		root: {
			type: "branch",
			data: [{ type: "leaf", data: { views: ["a"], activeView: "a", id: "1" }, size: 800 }],
			size: 600,
		},
		width: 800,
		height: 600,
		orientation: "HORIZONTAL",
	},
	panels: { a: { id: "a", contentComponent: "agent", params: { agent: "a" }, title: "A" } },
	activeGroup: "1",
} as SerializedDockview;

afterEach(() => {
	vi.restoreAllMocks();
});

describe("pane layout storage", () => {
	it("gives back what a window saved, and keeps windows apart", () => {
		saveLayout("main", LAYOUT);
		expect(loadLayout("main")).toEqual(LAYOUT);
		expect(loadLayout("second")).toBeNull();
	});

	it("drops a saved layout it can't read", () => {
		for (const raw of [
			"{not json",
			JSON.stringify({ version: 99, dock: LAYOUT }),
			JSON.stringify({ version: 1, dock: { grid: null, panels: {} } }),
			JSON.stringify({
				version: 1,
				dock: { ...LAYOUT, panels: { a: { id: "b", contentComponent: "agent" } } },
			}),
		]) {
			localStorage.setItem(layoutKey("main"), raw);
			expect(loadLayout("main")).toBeNull();
			expect(localStorage.getItem(layoutKey("main"))).toBeNull();
		}
	});

	it("reads no layout when storage throws, and saving there does no harm", () => {
		vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
			throw new Error("blocked");
		});
		vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
			throw new Error("full");
		});
		const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
		expect(loadLayout("main")).toBeNull();
		expect(() => saveLayout("main", LAYOUT)).not.toThrow();
		expect(warn).toHaveBeenCalled();
	});
});
