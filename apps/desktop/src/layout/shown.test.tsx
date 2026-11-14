// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, render } from "@testing-library/react";
import { type DockviewApi, DockviewReact } from "dockview-react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { Services } from "@/app/services";
import type { DataSource } from "@maxhq/shared/data/source";
import type { Activity, Shown } from "@maxhq/shared/bindings";
import { agent, entities } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";
import { applySnapshot } from "@maxhq/shared/view/replica";
import { createViewStore } from "@maxhq/shared/view/store";

import { AGENT_PANE, panesOf } from "./panes";
import { type DockLike, type GroupLike, shownIn, visibleAgents, watchShown } from "./shown";

// A group as `visibleAgents` reads it.
const group = (
	agent: unknown,
	{ visible = true, location = "grid" }: { visible?: boolean; location?: string } = {}
): GroupLike => ({
	activePanel: agent === undefined ? undefined : { params: { agent } },
	api: { isVisible: visible, location: { type: location } },
});

const ON_SCREEN = { focused: true, visible: true };

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

const pane = (
	api: DockviewApi,
	agent: string,
	position?: Parameters<DockviewApi["addPanel"]>[0]["position"]
) =>
	api.addPanel({
		id: agent,
		component: AGENT_PANE,
		tabComponent: AGENT_PANE,
		params: { agent },
		position,
	});

afterEach(() => {
	vi.restoreAllMocks();
});

describe("visibleAgents", () => {
	it("lists the agent in each group's active pane once, in order", () => {
		const shown: DockLike = {
			groups: [group("c"), group("a"), group("c")],
			activeGroup: undefined,
		};
		expect(visibleAgents(shown)).toEqual(["a", "c"]);
	});

	it("counts a floating group", () => {
		const shown: DockLike = {
			groups: [group("a"), group("b", { location: "floating" })],
			activeGroup: undefined,
		};
		expect(visibleAgents(shown)).toEqual(["a", "b"]);
	});

	it("skips a group that's hidden, popped out, empty or not an agent's", () => {
		const shown: DockLike = {
			groups: [
				group("a", { visible: false }),
				group("b", { location: "popout" }),
				group(undefined),
				group(42),
				group("e"),
			],
			activeGroup: undefined,
		};
		expect(visibleAgents(shown)).toEqual(["e"]);
	});
});

describe("shownIn", () => {
	const focusedOnB: DockLike = { groups: [group("a"), group("b")], activeGroup: group("b") };

	it("has a focused pane only while the window has focus", () => {
		expect(shownIn(focusedOnB, ON_SCREEN)).toEqual({ focused: "b", visible: ["a", "b"] });
		expect(shownIn(focusedOnB, { focused: false, visible: true })).toEqual({
			focused: null,
			visible: ["a", "b"],
		});
	});

	it("shows nothing from a window off screen or with no panes", () => {
		const nothing: Shown = { focused: null, visible: [] };
		expect(shownIn(focusedOnB, { focused: true, visible: false })).toEqual(nothing);
		expect(shownIn(null, ON_SCREEN)).toEqual(nothing);
	});

	it("has no focused pane when the active group is hidden", () => {
		const shown: DockLike = {
			groups: [group("a")],
			activeGroup: group("b", { visible: false }),
		};
		expect(shownIn(shown, ON_SCREEN)).toEqual({ focused: null, visible: ["a"] });
	});
});

describe("what a real layout shows", () => {
	it("shows the active pane of each split, not a tab behind one", () => {
		const api = dock();
		act(() => {
			pane(api, "a");
			pane(api, "b");
			pane(api, "c", { direction: "right" });
		});
		expect(shownIn(api, ON_SCREEN)).toEqual({ focused: "c", visible: ["b", "c"] });

		act(() => api.getPanel("a")?.api.setActive());
		expect(shownIn(api, ON_SCREEN)).toEqual({ focused: "a", visible: ["a", "c"] });
	});

	it("shows a floating pane", () => {
		const api = dock();
		act(() => {
			pane(api, "a");
			pane(api, "b");
		});
		act(() => {
			const b = api.getPanel("b");
			if (b) api.addFloatingGroup(b);
		});
		expect(api.groups).toHaveLength(2);
		expect(shownIn(api, ON_SCREEN)).toEqual({ focused: "b", visible: ["a", "b"] });
	});

	it("shows only the maximized group while one is", () => {
		const api = dock();
		act(() => {
			pane(api, "a");
			pane(api, "b", { direction: "right" });
		});
		act(() => {
			const b = api.getPanel("b");
			if (b) api.maximizeGroup(b);
		});
		expect(shownIn(api, ON_SCREEN)).toEqual({ focused: "b", visible: ["b"] });

		act(() => api.exitMaximizedGroup());
		expect(shownIn(api, ON_SCREEN)).toEqual({ focused: "b", visible: ["a", "b"] });
	});
});

describe("watchShown", () => {
	const DONE: Activity = { state: "done_unread" };

	const setup = () => {
		const setShown = vi.fn<DataSource["setShown"]>(async () => {});
		const markSeen = vi.fn<DataSource["markSeen"]>(async () => ({ status: "ok", data: null }));
		const source = { kind: "mock", setShown, markSeen } as unknown as DataSource;
		const view = createViewStore();
		view.setState({
			view: applySnapshot({
				seq: 1,
				entities: entities({
					agents: [agent({ id: "a" }), agent({ id: "b", activity: DONE })],
				}),
			}),
			status: { state: "ready" },
		});
		const services: Services = { source, view, ui: createUiStore() };
		return { services, setShown, markSeen };
	};

	// Lets the queued report run.
	const settle = () => act(async () => {});

	it("reports once per change and not again when nothing changed", async () => {
		vi.spyOn(document, "hasFocus").mockReturnValue(true);
		const { services, setShown } = setup();
		const api = dock();
		const stop = watchShown(services);
		const detach = panesOf(services).attach(api);
		await settle();
		expect(setShown.mock.calls).toEqual([[{ focused: null, visible: [] }]]);

		act(() => {
			pane(api, "a");
			pane(api, "b", { direction: "right" });
		});
		await settle();
		expect(setShown.mock.calls.at(-1)).toEqual([{ focused: "b", visible: ["a", "b"] }]);
		const count = setShown.mock.calls.length;

		window.dispatchEvent(new Event("focus"));
		await settle();
		expect(setShown).toHaveBeenCalledTimes(count);

		detach();
		stop();
	});

	it("follows the window's focus, its visibility and the devices view", async () => {
		const focus = vi.spyOn(document, "hasFocus").mockReturnValue(true);
		const { services, setShown } = setup();
		const api = dock();
		const stop = watchShown(services);
		const detach = panesOf(services).attach(api);
		act(() => {
			pane(api, "a");
		});
		await settle();
		expect(setShown.mock.calls.at(-1)).toEqual([{ focused: "a", visible: ["a"] }]);

		focus.mockReturnValue(false);
		window.dispatchEvent(new Event("blur"));
		await settle();
		expect(setShown.mock.calls.at(-1)).toEqual([{ focused: null, visible: ["a"] }]);

		const hidden = vi.spyOn(document, "visibilityState", "get").mockReturnValue("hidden");
		document.dispatchEvent(new Event("visibilitychange"));
		await settle();
		expect(setShown.mock.calls.at(-1)).toEqual([{ focused: null, visible: [] }]);

		hidden.mockReturnValue("visible");
		document.dispatchEvent(new Event("visibilitychange"));
		await settle();
		expect(setShown.mock.calls.at(-1)).toEqual([{ focused: null, visible: ["a"] }]);

		act(() => services.ui.getState().setMode("devices"));
		await settle();
		expect(setShown.mock.calls.at(-1)).toEqual([{ focused: null, visible: [] }]);

		detach();
		stop();
	});

	it("marks the agent in the focused pane seen, not one that only shows", async () => {
		const focus = vi.spyOn(document, "hasFocus").mockReturnValue(false);
		const { services, markSeen } = setup();
		const api = dock();
		const stop = watchShown(services);
		const detach = panesOf(services).attach(api);
		act(() => {
			pane(api, "b");
			pane(api, "a", { direction: "right" });
		});
		await settle();
		expect(markSeen).not.toHaveBeenCalled();

		focus.mockReturnValue(true);
		window.dispatchEvent(new Event("focus"));
		await settle();
		expect(markSeen).not.toHaveBeenCalled();

		act(() => api.getPanel("b")?.api.setActive());
		await settle();
		expect(markSeen.mock.calls).toEqual([["b"]]);

		detach();
		stop();
	});

	it("stops reporting once stopped", async () => {
		vi.spyOn(document, "hasFocus").mockReturnValue(true);
		const { services, setShown } = setup();
		const api = dock();
		const stop = watchShown(services);
		const detach = panesOf(services).attach(api);
		await settle();
		const count = setShown.mock.calls.length;

		stop();
		act(() => {
			pane(api, "a");
		});
		window.dispatchEvent(new Event("blur"));
		await settle();
		expect(setShown).toHaveBeenCalledTimes(count);
		detach();
	});
});
