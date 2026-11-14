// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, render } from "@testing-library/react";
import { type DockviewApi, DockviewReact } from "dockview-react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { closePane, cycleTab, focusGroup, showTab, splitGroup, toggleMaximized } from "./commands";
import { AGENT_PANE } from "./panes";

// A real dockview instance with panes that render nothing.
const dock = (): DockviewApi => {
	let api: DockviewApi | undefined;
	render(
		<DockviewReact
			components={{ [AGENT_PANE]: () => null }}
			tabComponents={{ [AGENT_PANE]: () => null }}
			dndStrategy="pointer"
			onReady={(event) => {
				api = event.api;
			}}
		/>
	);
	if (!api) throw new Error("dockview didn't start");
	return api;
};

const add = (api: DockviewApi, id: string, beside?: string, direction = "within") =>
	act(() => {
		api.addPanel({
			id,
			component: AGENT_PANE,
			title: id,
			position: beside
				? { referencePanel: beside, direction: direction as "within" | "right" | "below" }
				: undefined,
		});
	});

// Puts the group holding `panel` at a box on screen, since jsdom lays nothing out.
const place = (api: DockviewApi, panel: string, left: number, top: number, w: number, h: number) =>
	vi.spyOn(api.getPanel(panel)!.group.element, "getBoundingClientRect").mockReturnValue({
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

const run = (command: () => boolean) => {
	let did = false;
	act(() => {
		did = command();
	});
	return did;
};

afterEach(() => {
	vi.restoreAllMocks();
});

describe("moving focus between groups", () => {
	// One group on the left, and two stacked on the right.
	const threeGroups = () => {
		const api = dock();
		add(api, "a");
		add(api, "b", "a", "right");
		add(api, "c", "b", "below");
		place(api, "a", 0, 0, 400, 400);
		place(api, "b", 404, 0, 400, 298);
		place(api, "c", 404, 302, 400, 298);
		return api;
	};

	it("goes to the group on that side, the nearest in line first", () => {
		const api = threeGroups();
		act(() => api.getPanel("a")!.api.setActive());
		expect(run(() => focusGroup(api, "right"))).toBe(true);
		expect(api.activePanel?.id).toBe("b");
		expect(run(() => focusGroup(api, "down"))).toBe(true);
		expect(api.activePanel?.id).toBe("c");
		expect(run(() => focusGroup(api, "left"))).toBe(true);
		expect(api.activePanel?.id).toBe("a");
	});

	it("leaves the content of the pane it goes to in the page", () => {
		const api = threeGroups();
		act(() => api.getPanel("a")!.api.setActive());
		const moves = new MutationObserver(() => {});
		moves.observe(document.body, { childList: true, subtree: true });
		expect(run(() => focusGroup(api, "right"))).toBe(true);
		const removed = moves.takeRecords().flatMap((r) => [...r.removedNodes]);
		moves.disconnect();
		expect(removed).not.toContain(api.getPanel("b")!.view.content.element);
	});

	it("stays put at the edge", () => {
		const api = threeGroups();
		act(() => api.getPanel("a")!.api.setActive());
		expect(run(() => focusGroup(api, "left"))).toBe(false);
		expect(run(() => focusGroup(api, "up"))).toBe(false);
		expect(api.activePanel?.id).toBe("a");
	});
});

describe("switching tabs", () => {
	const oneGroup = () => {
		const api = dock();
		add(api, "a");
		add(api, "b", "a");
		add(api, "c", "a");
		return api;
	};

	it("goes round the tabs either way", () => {
		const api = oneGroup();
		expect(api.activePanel?.id).toBe("c");
		run(() => cycleTab(api, 1));
		expect(api.activePanel?.id).toBe("a");
		run(() => cycleTab(api, -1));
		expect(api.activePanel?.id).toBe("c");
		run(() => cycleTab(api, -1));
		expect(api.activePanel?.id).toBe("b");
	});

	it("shows a tab by its number, with 9 for the last", () => {
		const api = oneGroup();
		expect(run(() => showTab(api, 1))).toBe(true);
		expect(api.activePanel?.id).toBe("a");
		expect(run(() => showTab(api, 9))).toBe(true);
		expect(api.activePanel?.id).toBe("c");
		expect(run(() => showTab(api, 5))).toBe(false);
	});
});

describe("splitting, maximizing and closing", () => {
	it("moves the visible tab into a new group beside its own", () => {
		const api = dock();
		add(api, "a");
		add(api, "b", "a");
		expect(run(() => splitGroup(api, api.activeGroup, "below"))).toBe(true);
		expect(api.groups).toHaveLength(2);
		expect(api.getPanel("b")!.group).not.toBe(api.getPanel("a")!.group);
		expect(api.activePanel?.id).toBe("b");
		// A group with one tab has nothing to split off.
		expect(run(() => splitGroup(api, api.activeGroup, "right"))).toBe(false);
	});

	it("doesn't split or maximize a floating group", () => {
		const api = dock();
		add(api, "a");
		add(api, "b", "a");
		add(api, "c", "a");
		act(() => {
			api.addFloatingGroup(api.getPanel("b")!);
			api.getPanel("c")!.api.moveTo({ group: api.getPanel("b")!.group });
		});
		const floating = api.getPanel("c")!.group;
		expect(floating.api.location.type).toBe("floating");
		expect(run(() => splitGroup(api, floating, "right"))).toBe(false);
		expect(run(() => toggleMaximized(floating))).toBe(false);
	});

	it("maximizes a group and restores it", () => {
		const api = dock();
		add(api, "a");
		add(api, "b", "a", "right");
		const group = api.getPanel("b")!.group;
		run(() => toggleMaximized(group));
		expect(api.hasMaximizedGroup()).toBe(true);
		run(() => toggleMaximized(group));
		expect(api.hasMaximizedGroup()).toBe(false);
	});

	it("closes the focused pane", () => {
		const api = dock();
		add(api, "a");
		add(api, "b", "a");
		expect(run(() => closePane(api))).toBe(true);
		expect(api.panels.map((p) => p.id)).toEqual(["a"]);
	});
});
