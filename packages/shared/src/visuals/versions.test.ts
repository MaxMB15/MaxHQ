// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { ItemId, TimelineItem } from "../bindings";
import { timelineItem } from "../timeline/build";
import { replacedLine, versionLabel, visualVersions } from "./versions";

const visual = (id: string, ordinal: number, replaces: string | null = null, turn = "t1") =>
	timelineItem(
		{
			kind: "agent_visual",
			title: "Weekly usage",
			attachment: {
				id: `att-${id}`,
				name: "visual-weekly-usage.html",
				kind: "file",
				bytes: 10,
				mime: "text/html",
				path: `/staged/${id}.html`,
				thumbnail: null,
			},
			replaces,
		},
		{ id, ordinal, turn }
	);

const itemsOf = (...items: TimelineItem[]): ReadonlyMap<ItemId, TimelineItem> =>
	new Map(items.map((item) => [item.id, item]));

describe("visualVersions", () => {
	it("counts versions along the chain and names what replaced each", () => {
		const items = itemsOf(visual("a", 0), visual("b", 1, "a"), visual("c", 2, "b", "t2"));
		const versions = visualVersions(items);
		expect(["a", "b", "c"].map(versions.version)).toEqual([1, 2, 3]);
		expect(["a", "b", "c"].map(versions.replacedBy)).toEqual(["b", "c", null]);
		expect(versions.turnHasVisual("t1")).toBe(true);
		expect(versions.turnHasVisual("t2")).toBe(true);
		expect(versions.turnHasVisual("t3")).toBe(false);
		// One index for one map.
		expect(visualVersions(items)).toBe(versions);
	});

	it("takes the newest of two replacements and ignores one that names no visual", () => {
		const versions = visualVersions(
			itemsOf(
				visual("a", 0),
				visual("c", 2, "a"),
				visual("b", 1, "a"),
				visual("d", 3, "gone")
			)
		);
		expect(versions.replacedBy("a")).toBe("c");
		expect(versions.version("d")).toBe(1);
		expect(versions.replacedBy("d")).toBeNull();
	});

	it("stops on a chain that loops", () => {
		const versions = visualVersions(itemsOf(visual("a", 0, "b"), visual("b", 1, "a")));
		expect(versions.version("a")).toBe(1000);
	});

	it("labels the version and the folded line", () => {
		expect(versionLabel(1)).toBeNull();
		expect(versionLabel(3)).toBe("v3");
		expect(replacedLine("Weekly usage", 2)).toBe(
			"Weekly usage, v2 · replaced by the one below"
		);
		expect(replacedLine("", 1)).toBe("Visual, first version · replaced by the one below");
	});
});
