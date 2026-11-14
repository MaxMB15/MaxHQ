// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, render } from "@testing-library/react";
import { type DockviewApi, DockviewReact } from "dockview-react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { sideAt, targetAt } from "./drop";
import { AGENT_PANE } from "./panes";

// A real dockview instance with panes that render nothing, and its grid.
const dock = (): { api: DockviewApi; root: HTMLElement } => {
	let api: DockviewApi | undefined;
	const { container } = render(
		<DockviewReact
			components={{ [AGENT_PANE]: () => null }}
			tabComponents={{ [AGENT_PANE]: () => null }}
			dndStrategy="pointer"
			onReady={(event) => {
				api = event.api;
			}}
		/>
	);
	const root = container.querySelector<HTMLElement>(".dv-dockview");
	if (!api || !root) throw new Error("dockview didn't start");
	return { api, root };
};

const add = (api: DockviewApi, id: string, beside?: string) =>
	act(() => {
		api.addPanel({
			id,
			component: AGENT_PANE,
			title: id,
			position: beside ? { referencePanel: beside, direction: "right" } : undefined,
		});
	});

// Puts an element at a box on screen, since jsdom lays nothing out.
const place = (element: Element, left: number, top: number, w: number, h: number) =>
	vi.spyOn(element, "getBoundingClientRect").mockReturnValue({
		left,
		top,
		x: left,
		y: top,
		width: w,
		height: h,
		right: left + w,
		bottom: top + h,
		toJSON: () => ({}),
	});

const groupOf = (api: DockviewApi, panel: string) => api.getPanel(panel)!.group.element;

afterEach(() => {
	vi.restoreAllMocks();
});

describe("which side of a group a point is on", () => {
	const box = { left: 100, top: 100, width: 200, height: 100 };

	it("splits from the outer fifth and stacks in the middle", () => {
		expect(sideAt(box, 130, 150)).toBe("left");
		expect(sideAt(box, 270, 150)).toBe("right");
		expect(sideAt(box, 200, 110)).toBe("top");
		expect(sideAt(box, 200, 190)).toBe("bottom");
		expect(sideAt(box, 200, 150)).toBe("center");
	});

	it("lets the sides win over the top and bottom in a corner", () => {
		expect(sideAt(box, 105, 105)).toBe("left");
		expect(sideAt(box, 295, 195)).toBe("right");
	});
});

describe("where a drop lands", () => {
	// Two groups side by side with a gap between them.
	const twoGroups = () => {
		const { api, root } = dock();
		add(api, "a");
		add(api, "b", "a");
		place(root, 0, 0, 804, 400);
		place(groupOf(api, "a"), 0, 0, 400, 400);
		place(groupOf(api, "b"), 404, 0, 400, 400);
		return { api, root };
	};
	const groupId = (api: DockviewApi, panel: string) => api.getPanel(panel)!.group.id;

	it("adds a tab in a group's middle and splits it at its sides", () => {
		const { api, root } = twoGroups();
		expect(targetAt(api, root, 200, 200)).toEqual({
			spot: { group: groupId(api, "a"), side: "center" },
			box: { left: 0, top: 0, width: 400, height: 400 },
		});
		expect(targetAt(api, root, 50, 200)).toEqual({
			spot: { group: groupId(api, "a"), side: "left" },
			box: { left: 0, top: 0, width: 200, height: 400 },
		});
		expect(targetAt(api, root, 780, 200)).toEqual({
			spot: { group: groupId(api, "b"), side: "right" },
			box: { left: 604, top: 0, width: 200, height: 400 },
		});
	});

	it("adds a tab when the point is on a group's tab bar", () => {
		const { api, root } = twoGroups();
		const bar = groupOf(api, "a").querySelector(".dv-tabs-and-actions-container")!;
		place(bar, 0, 0, 400, 32);
		expect(targetAt(api, root, 200, 20)).toEqual({
			spot: { group: groupId(api, "a"), side: "center" },
			box: { left: 0, top: 32, width: 400, height: 368 },
		});
	});

	it("splits the whole grid at its outer edge", () => {
		const { api, root } = twoGroups();
		expect(targetAt(api, root, 4, 200)).toEqual({
			spot: { group: null, side: "left" },
			box: { left: 0, top: 0, width: 20, height: 400 },
		});
		expect(targetAt(api, root, 600, 396)).toEqual({
			spot: { group: null, side: "bottom" },
			box: { left: 0, top: 380, width: 804, height: 20 },
		});
	});

	it("lands nowhere between groups or outside the grid", () => {
		const { api, root } = twoGroups();
		expect(targetAt(api, root, 402, 200)).toBeNull();
		expect(targetAt(api, root, 900, 200)).toBeNull();
	});

	it("only adds tabs to a floating group, which sits above the grid", () => {
		const { api, root } = twoGroups();
		add(api, "f");
		act(() => {
			api.addFloatingGroup(api.getPanel("f")!);
		});
		place(groupOf(api, "f"), 100, 100, 200, 150);
		expect(targetAt(api, root, 105, 200)).toEqual({
			spot: { group: groupId(api, "f"), side: "center" },
			box: { left: 100, top: 100, width: 200, height: 150 },
		});
	});

	it("fills an empty grid", () => {
		const { api, root } = dock();
		place(root, 0, 0, 800, 600);
		expect(targetAt(api, root, 400, 300)).toEqual({
			spot: { group: null, side: "center" },
			box: { left: 0, top: 0, width: 800, height: 600 },
		});
	});
});
