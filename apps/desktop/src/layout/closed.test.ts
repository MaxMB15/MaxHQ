// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { Orientation, type SerializedDockview } from "dockview-react";
import { describe, expect, it } from "vitest";

import { placeOf } from "./closed";

const leaf = (id: string, views: string[], size = 100) => ({
	type: "leaf" as const,
	data: { id, views, activeView: views[0] },
	size,
});

const layout = (
	data: unknown[],
	floatingGroups: SerializedDockview["floatingGroups"] = []
): SerializedDockview =>
	({
		grid: {
			root: { type: "branch", data, size: 600 },
			width: 900,
			height: 600,
			orientation: Orientation.HORIZONTAL,
		},
		panels: {},
		floatingGroups,
	}) as SerializedDockview;

describe("where a closed pane was", () => {
	it("keeps its group, its place among the tabs and the group to its left", () => {
		const place = placeOf(layout([leaf("1", ["a"]), leaf("2", ["b", "c"], 300)]), "c");
		expect(place).toEqual({
			group: "2",
			index: 1,
			floating: null,
			beside: { group: "1", side: "right", size: 300 },
		});
	});

	it("splits from the group to its right when it was first", () => {
		const place = placeOf(layout([leaf("1", ["a"], 250), leaf("2", ["b"])]), "a");
		expect(place?.beside).toEqual({ group: "2", side: "left", size: 250 });
	});

	it("splits above or below inside a column", () => {
		const column = {
			type: "branch",
			data: [leaf("2", ["b"]), leaf("3", ["c"], 180)],
			size: 400,
		};
		const place = placeOf(layout([leaf("1", ["a"]), column]), "c");
		expect(place?.beside).toEqual({ group: "2", side: "below", size: 180 });
	});

	it("takes the grid's edge when it sat at the end after a column", () => {
		const column = { type: "branch", data: [leaf("2", ["b"]), leaf("3", ["c"])], size: 400 };
		const place = placeOf(layout([column, leaf("1", ["a"], 220)]), "a");
		expect(place?.beside).toEqual({ group: null, side: "right", size: 220 });
	});

	it("keeps the box a floating group sat in", () => {
		const floating = {
			data: { id: "9", views: ["f"], activeView: "f" },
			position: { left: 40, top: 30, width: 320, height: 240 },
		};
		expect(placeOf(layout([leaf("1", ["a"])], [floating]), "f")).toEqual({
			group: "9",
			index: 0,
			floating: { position: { left: 40, top: 30 }, width: 320, height: 240 },
			beside: null,
		});
	});

	it("has nothing for an agent without a pane", () => {
		expect(placeOf(layout([leaf("1", ["a"])]), "z")).toBeNull();
	});
});
