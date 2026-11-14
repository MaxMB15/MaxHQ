// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, render } from "@testing-library/react";
import { type DockviewApi, DockviewReact } from "dockview-react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { Services } from "@/app/services";
import { mockSource } from "@/data/mock-source";
import { paneId } from "@/layout/pane-id";
import { AGENT_PANE, FILE_PANE, panesOf } from "@/layout/panes";
import type { TimelineItem, TimelineUpdate } from "@maxhq/shared/bindings";
import type { DataSource } from "@maxhq/shared/data/source";
import { timelineItem } from "@maxhq/shared/timeline/build";
import { openTimeline } from "@maxhq/shared/timeline/store";
import { createUiStore } from "@maxhq/shared/ui/store";
import { createViewStore } from "@maxhq/shared/view/store";

import { clearReveals, revealed } from "@/viewer/reveal";

import { forgetOpened, openArrived } from "./auto-open";

// A real dockview instance with panes that render nothing.
const dock = (): DockviewApi => {
	let api: DockviewApi | undefined;
	render(
		<DockviewReact
			components={{ [AGENT_PANE]: () => null, [FILE_PANE]: () => null }}
			tabComponents={{ [AGENT_PANE]: () => null, [FILE_PANE]: () => null }}
			dndStrategy="pointer"
			onReady={(event) => {
				api = event.api;
			}}
		/>
	);
	if (!api) throw new Error("dockview didn't start");
	return api;
};

const PLAN = "/tmp/example/docs/plan.md";
const FILE = paneId({ kind: "file", agent: "a", path: PLAN });

const opened = (id: string, ordinal: number, started_at: number | null = null): TimelineItem =>
	timelineItem(
		{ kind: "agent_opened_file", path: PLAN, line: 3, outside: false },
		{ id, ordinal, agent: "a", started_at }
	);

// A window with agents `a` and `b` as tabs of one group, `a` showing, and a
// timeline for `a` whose updates `push` sends.
const setup = () => {
	let push: (update: TimelineUpdate, live: boolean) => void = () => {};
	const source = {
		...mockSource({ tick: false }),
		openTimeline: async (_agent: string, onUpdate: typeof push) => {
			push = onUpdate;
			return () => {};
		},
	} satisfies DataSource;
	const services: Services = { source, view: createViewStore(), ui: createUiStore() };
	const api = dock();
	panesOf(services).attach(api);
	act(() => {
		panesOf(services).open("b");
		panesOf(services).open("a");
	});
	const timeline = openTimeline(source, "a", { schedule: (flush) => flush() });
	timeline.onArrive((items) => openArrived(services, "a", items));
	return {
		api,
		services,
		push: (update: TimelineUpdate, live: boolean) => act(() => push(update, live)),
		ready: () => act(async () => {}),
	};
};

const upsert = (seq: number, item: TimelineItem): TimelineUpdate => ({
	type: "events",
	seq,
	events: [{ type: "upsert", item }],
});

let focused: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
	forgetOpened();
	clearReveals();
	focused = vi.spyOn(document, "hasFocus").mockReturnValue(true);
});

afterEach(() => {
	vi.restoreAllMocks();
});

describe("opening a file the agent opened for you", () => {
	it("opens a live one as a tab in the agent's group", async () => {
		const { api, push, ready } = setup();
		await ready();
		push({ type: "reset", seq: 1, items: [], has_older: false }, false);
		push(upsert(2, opened("o1", 0)), true);
		const file = api.getPanel(FILE);
		expect(file).toBeDefined();
		expect(file?.group).toBe(api.getPanel("a")?.group);
		expect(api.groups).toHaveLength(1);
		expect(api.activePanel?.id).toBe(FILE);
		expect(revealed("a", PLAN)?.line).toBe(3);
	});

	it("opens one that began just now, but not one from long ago", async () => {
		const { api, push, ready } = setup();
		await ready();
		push({ type: "reset", seq: 1, items: [], has_older: false }, false);
		push(upsert(2, opened("old", 0, Date.now() - 60 * 60 * 1000)), true);
		expect(api.getPanel(FILE)).toBeUndefined();
		push(upsert(3, opened("new", 1, Date.now() - 1000)), true);
		expect(api.getPanel(FILE)).toBeDefined();
	});

	it("doesn't open one from a first load, a caught-up change or an older page", async () => {
		const { api, push, ready } = setup();
		await ready();
		push({ type: "reset", seq: 1, items: [opened("o1", 0)], has_older: true }, false);
		push(upsert(2, opened("o2", 1)), false);
		push({ type: "older", items: [opened("o0", -1)], has_older: false }, false);
		expect(api.getPanel(FILE)).toBeUndefined();
	});

	it("doesn't open one while the agent's tab is behind another", async () => {
		const { api, push, ready } = setup();
		await ready();
		push({ type: "reset", seq: 1, items: [], has_older: false }, false);
		act(() => api.getPanel("b")?.api.setActive());
		push(upsert(2, opened("o1", 0)), true);
		expect(api.getPanel(FILE)).toBeUndefined();
	});

	it("doesn't open one while the window is in the background", async () => {
		const { api, push, ready } = setup();
		await ready();
		push({ type: "reset", seq: 1, items: [], has_older: false }, false);
		focused.mockReturnValue(false);
		push(upsert(2, opened("o1", 0)), true);
		expect(api.getPanel(FILE)).toBeUndefined();
	});

	it("opens each item once", async () => {
		const { api, push, ready } = setup();
		await ready();
		push({ type: "reset", seq: 1, items: [], has_older: false }, false);
		push(upsert(2, opened("o1", 0)), true);
		act(() => api.getPanel(FILE)?.api.close());
		expect(api.getPanel(FILE)).toBeUndefined();

		// The same item again, as a reconnect that lost it would send it.
		push({ type: "reset", seq: 3, items: [], has_older: false }, false);
		push(upsert(4, opened("o1", 0)), true);
		expect(api.getPanel(FILE)).toBeUndefined();
	});
});
