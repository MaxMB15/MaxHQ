// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, render } from "@testing-library/react";
import { type DockviewApi, DockviewReact } from "dockview-react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { RunKey } from "@maxhq/shared/bindings";
import { revealed } from "@/viewer/reveal";
import { clearVisualPages, visualPage } from "@/visuals/tab-pages";
import { visualKey } from "@maxhq/shared/visuals/visual";

import { paneAgent, paneId, type PaneParams, paneParams } from "./pane-id";
import {
	AGENT_PANE,
	createPanes,
	FILE_PANE,
	keepLayout,
	NOTE_PANE,
	pageKept,
	restoreLayout,
	VISUAL_PANE,
	WORKFLOW_PANE,
} from "./panes";
import { layoutKey, loadLayout } from "./storage";

// A real dockview instance with panes that render nothing.
const dock = (): DockviewApi => {
	let api: DockviewApi | undefined;
	render(
		<DockviewReact
			components={{
				[AGENT_PANE]: () => null,
				[WORKFLOW_PANE]: () => null,
				[NOTE_PANE]: () => null,
				[FILE_PANE]: () => null,
				[VISUAL_PANE]: () => null,
			}}
			tabComponents={{
				[AGENT_PANE]: () => null,
				[WORKFLOW_PANE]: () => null,
				[NOTE_PANE]: () => null,
				[FILE_PANE]: () => null,
				[VISUAL_PANE]: () => null,
			}}
			dndStrategy="pointer"
			onReady={(event) => {
				api = event.api;
			}}
		/>
	);
	if (!api) throw new Error("dockview didn't start");
	return api;
};

// A pane's title: its agent, its run's ID, its note's ID or its file's path.
const named = (params: PaneParams) => {
	if (params.kind === "agent") return params.agent;
	if (params.kind === "workflow") return params.run.run;
	if (params.kind === "note") return params.note;
	if (params.kind === "visual") return params.title;
	return params.path;
};
// Whether a pane's agent isn't `gone`.
const not = (gone: string) => (params: PaneParams) => paneAgent(paneId(params)) !== gone;

const RUN: RunKey = { device: "mac", project: "p/1", workflow: "pipeline", run: "r 1" };

const ids = (api: DockviewApi) => api.panels.map((p) => p.id).sort();

// The agents in each top-level part of the grid, left to right.
const columns = (api: DockviewApi) =>
	(api.toJSON().grid.root.data as { data: { views: string[] } }[]).map((n) => n.data.views);

afterEach(() => {
	vi.restoreAllMocks();
	clearVisualPages();
});

describe("opening panes", () => {
	it("focuses the pane an agent already has instead of opening another", () => {
		const api = dock();
		const panes = createPanes((params) => `Agent ${named(params)}`);
		panes.attach(api);

		act(() => panes.open("a"));
		act(() => panes.open("b"));
		expect(ids(api)).toEqual(["a", "b"]);
		expect(api.groups).toHaveLength(1);
		expect(api.activePanel?.id).toBe("b");

		act(() => panes.open("a"));
		expect(ids(api)).toEqual(["a", "b"]);
		expect(api.activePanel?.id).toBe("a");
		expect(api.getPanel("a")?.title).toBe("Agent a");
	});

	it("focuses a pane that shows in another split without taking its content out", () => {
		const api = dock();
		const panes = createPanes(named);
		panes.attach(api);
		act(() => panes.open("a"));
		act(() => panes.open("b", { split: true }));
		act(() => panes.open("c", { split: true }));
		expect(api.groups).toHaveLength(3);
		const moves = new MutationObserver(() => {});
		moves.observe(document.body, { childList: true, subtree: true });
		const removed = () => moves.takeRecords().flatMap((r) => [...r.removedNodes]);

		for (const id of ["a", "b", "a", "c"]) {
			const pane = api.getPanel(id)!;
			act(() => panes.open(id));
			expect(api.activePanel?.id).toBe(id);
			expect(api.activeGroup).toBe(pane.group);
			expect(removed()).not.toContain(pane.view.content.element);
		}
		moves.disconnect();
	});

	it("opens a split in a new group beside the focused one", () => {
		const api = dock();
		const panes = createPanes(named);
		panes.attach(api);
		act(() => panes.open("a"));
		act(() => panes.open("b", { split: true }));
		expect(api.groups).toHaveLength(2);
		expect(api.activePanel?.id).toBe("b");
	});

	it("holds an open that comes before dockview is ready", () => {
		const panes = createPanes(named);
		panes.open("a");
		const api = dock();
		act(() => {
			panes.attach(api);
		});
		expect(api.activePanel?.id).toBe("a");
	});
});

describe("dropping an agent on the panes", () => {
	const ready = () => {
		const api = dock();
		const panes = createPanes(named);
		panes.attach(api);
		return { api, panes };
	};
	const groupOf = (api: DockviewApi, agent: string) => api.getPanel(agent)!.group.id;

	it("adds a tab in a group's middle", () => {
		const { api, panes } = ready();
		act(() => {
			panes.open("a");
			panes.open("b", { split: true });
		});
		act(() => panes.open("c", { at: { group: groupOf(api, "a"), side: "center" } }));
		expect(columns(api)).toEqual([["a", "c"], ["b"]]);
		expect(api.activePanel?.id).toBe("c");
	});

	it("opens a new group at a group's side or along the grid's side", () => {
		const { api, panes } = ready();
		act(() => panes.open("a"));
		act(() => panes.open("b", { at: { group: groupOf(api, "a"), side: "left" } }));
		expect(columns(api)).toEqual([["b"], ["a"]]);
		act(() => panes.open("c", { at: { group: null, side: "right" } }));
		expect(columns(api)).toEqual([["b"], ["a"], ["c"]]);
		expect(api.activePanel?.id).toBe("c");
	});

	it("moves a pane the agent already has", () => {
		const { api, panes } = ready();
		act(() => {
			panes.open("a");
			panes.open("b");
		});
		act(() => panes.open("b", { at: { group: groupOf(api, "a"), side: "right" } }));
		expect(columns(api)).toEqual([["a"], ["b"]]);
		act(() => panes.open("a", { at: { group: groupOf(api, "b"), side: "center" } }));
		expect(columns(api)).toEqual([["b", "a"]]);
		act(() => panes.open("a", { at: { group: null, side: "left" } }));
		expect(columns(api)).toEqual([["a"], ["b"]]);
	});

	it("leaves a pane alone in its group where it is", () => {
		const { api, panes } = ready();
		act(() => {
			panes.open("a");
			panes.open("b", { split: true });
		});
		const before = api.toJSON().grid;
		act(() => panes.open("a", { at: { group: groupOf(api, "a"), side: "left" } }));
		expect(api.toJSON().grid).toEqual(before);
		expect(api.activePanel?.id).toBe("a");
	});

	it("fills an empty grid and gets out of a maximized group to split", () => {
		const { api, panes } = ready();
		act(() => panes.open("a", { at: { group: null, side: "center" } }));
		expect(columns(api)).toEqual([["a"]]);
		act(() => {
			panes.open("b", { split: true });
			api.getPanel("b")!.group.api.maximize();
		});
		act(() => panes.open("c", { at: { group: groupOf(api, "b"), side: "right" } }));
		expect(api.hasMaximizedGroup()).toBe(false);
		expect(columns(api)).toEqual([["a"], ["b"], ["c"]]);
	});

	it("opens a tab in the focused group when the group it was dropped on closed", () => {
		const { api, panes } = ready();
		act(() => panes.open("a"));
		act(() => panes.open("b", { at: { group: "gone", side: "left" } }));
		expect(columns(api)).toEqual([["a", "b"]]);
	});
});

describe("reopening a closed pane", () => {
	const tabsOf = (api: DockviewApi, agent: string) =>
		api.getPanel(agent)?.group.panels.map((p) => p.id);
	const close = (api: DockviewApi, agent: string) => act(() => api.getPanel(agent)!.api.close());

	it("puts a pane back in its group at its place among the tabs", () => {
		const api = dock();
		const panes = createPanes(named);
		panes.attach(api);
		act(() => {
			panes.open("a");
			panes.open("b");
			panes.open("c");
		});
		close(api, "b");
		expect(panes.canReopen()).toBe(true);

		let reopened = false;
		act(() => {
			reopened = panes.reopen();
		});
		expect(reopened).toBe(true);
		expect(tabsOf(api, "b")).toEqual(["a", "b", "c"]);
		expect(api.activePanel?.id).toBe("b");
		expect(panes.canReopen()).toBe(false);
	});

	it("splits from the group it sat beside when its own group closed with it", () => {
		const api = dock();
		const panes = createPanes(named);
		panes.attach(api);
		act(() => {
			panes.open("a");
			panes.open("b", { split: true });
		});
		expect(columns(api)).toEqual([["a"], ["b"]]);

		close(api, "a");
		expect(api.groups).toHaveLength(1);
		act(() => {
			panes.reopen();
		});
		expect(columns(api)).toEqual([["a"], ["b"]]);

		close(api, "b");
		act(() => {
			panes.reopen();
		});
		expect(columns(api)).toEqual([["a"], ["b"]]);
	});

	it("follows a group that came back under a new ID", () => {
		const api = dock();
		const panes = createPanes(named);
		panes.attach(api);
		act(() => {
			panes.open("a");
			panes.open("b");
			panes.open("c", { split: true });
		});
		close(api, "a");
		close(api, "b");
		act(() => {
			panes.reopen();
			panes.reopen();
		});
		expect(tabsOf(api, "a")).toEqual(["a", "b"]);
		expect(columns(api)).toEqual([["a", "b"], ["c"]]);
	});

	it("floats a pane again when its group floated", () => {
		const api = dock();
		const panes = createPanes(named);
		panes.attach(api);
		act(() => {
			panes.open("a");
			panes.open("b");
		});
		act(() => {
			api.addFloatingGroup(api.getPanel("b")!, {
				position: { left: 40, top: 30 },
				width: 320,
				height: 240,
			});
		});
		close(api, "b");
		act(() => {
			panes.reopen();
		});
		expect(api.getPanel("b")!.group.api.location.type).toBe("floating");
	});

	it("passes over agents that are gone and panes that are open again", () => {
		const api = dock();
		const panes = createPanes(named, not("gone"));
		panes.attach(api);
		act(() => {
			panes.open("a");
			panes.open("b");
			panes.open("gone");
			panes.open("c");
		});
		close(api, "a");
		close(api, "b");
		close(api, "gone");
		act(() => panes.open("b"));

		let reopened = false;
		act(() => {
			reopened = panes.reopen();
		});
		expect(reopened).toBe(true);
		expect(ids(api)).toEqual(["a", "b", "c"]);
		expect(panes.canReopen()).toBe(false);
		act(() => {
			reopened = panes.reopen();
		});
		expect(reopened).toBe(false);
	});

	it("lists what it can put back once each, the last closed first", () => {
		const api = dock();
		const panes = createPanes(named, not("gone"));
		expect(panes.recentlyClosed()).toEqual([]);
		panes.attach(api);
		act(() => {
			for (const agent of ["a", "b", "gone", "c", "d"]) panes.open(agent);
		});
		close(api, "a");
		close(api, "b");
		close(api, "gone");
		close(api, "c");
		act(() => panes.open("a"));
		close(api, "a");
		act(() => panes.open("b"));

		const agents = () =>
			panes.recentlyClosed().map((params) => (params.kind === "agent" ? params.agent : null));
		expect(agents()).toEqual(["a", "c"]);
	});

	it("doesn't count panes that move to another group", () => {
		const api = dock();
		const panes = createPanes(named);
		panes.attach(api);
		act(() => {
			panes.open("a");
			panes.open("b");
		});
		act(() => {
			const beside = api.addGroup({ referenceGroup: api.activeGroup!, direction: "right" });
			api.getPanel("b")!.api.moveTo({ group: beside });
		});
		expect(api.groups).toHaveLength(2);
		expect(panes.canReopen()).toBe(false);
	});
});

describe("a run's pane", () => {
	it("has an ID no agent has, which gives its run back", () => {
		const id = paneId({ kind: "workflow", run: RUN });
		expect(paneParams(id)).toEqual({ kind: "workflow", run: RUN });
		expect(paneAgent(id)).toBeNull();
		expect(paneParams("a")).toEqual({ kind: "agent", agent: "a" });
	});

	it("opens once, beside agent panes, and reopens where it was", () => {
		const api = dock();
		const panes = createPanes(named);
		panes.attach(api);
		act(() => {
			panes.open("a");
			panes.openRun(RUN, { split: true });
			panes.openRun(RUN);
		});
		const id = paneId({ kind: "workflow", run: RUN });
		expect(ids(api)).toEqual(["a", id].sort());
		expect(api.getPanel(id)?.title).toBe("r 1");
		expect(api.getPanel(id)?.params).toEqual({ kind: "workflow", run: RUN });

		act(() => api.getPanel(id)!.api.close());
		expect(ids(api)).toEqual(["a"]);
		act(() => {
			panes.reopen();
		});
		expect(api.getPanel(id)?.group.id).not.toBe(api.getPanel("a")?.group.id);
		expect(api.groups).toHaveLength(2);
	});

	it("doesn't reopen when its run is gone", () => {
		const api = dock();
		const panes = createPanes(named, (params) => params.kind === "agent");
		panes.attach(api);
		act(() => panes.openRun(RUN));
		act(() => api.panels[0]!.api.close());
		expect(panes.canReopen()).toBe(false);
	});

	it("comes back with a saved layout", () => {
		const first = dock();
		const panes = createPanes(named);
		panes.attach(first);
		const stop = keepLayout(first, "main");
		act(() => {
			panes.open("a");
			panes.openRun(RUN, { split: true });
		});
		stop();

		const second = dock();
		act(() => restoreLayout(second, "main", () => true));
		const id = paneId({ kind: "workflow", run: RUN });
		expect(ids(second)).toEqual(["a", id].sort());
		expect(second.getPanel(id)?.api.component).toBe(WORKFLOW_PANE);
		expect(second.getPanel(id)?.params).toEqual({ kind: "workflow", run: RUN });
	});
});

describe("a note's pane", () => {
	it("has an ID that gives its project and note back, slashes and all", () => {
		const params = { kind: "note", project: "p/1", note: "n 1" } as const;
		const id = paneId(params);
		expect(paneParams(id)).toEqual(params);
		expect(paneAgent(id)).toBeNull();
	});

	it("opens once and comes back with a saved layout", () => {
		const first = dock();
		const panes = createPanes(named);
		panes.attach(first);
		const stop = keepLayout(first, "main");
		act(() => {
			panes.openNote("p/1", "n1");
			panes.openNote("p/1", "n1");
		});
		stop();
		const id = paneId({ kind: "note", project: "p/1", note: "n1" });
		expect(ids(first)).toEqual([id]);
		expect(first.getPanel(id)?.title).toBe("n1");

		const second = dock();
		act(() => restoreLayout(second, "main", () => true));
		expect(second.getPanel(id)?.api.component).toBe(NOTE_PANE);
	});
});

describe("a file's pane", () => {
	it("has an ID that gives its agent and path back, slashes and all", () => {
		const params = { kind: "file", agent: "a/1", path: "/w/docs/My plan.md" } as const;
		const id = paneId(params);
		expect(paneParams(id)).toEqual(params);
		expect(paneAgent(id)).toBeNull();
	});

	it("stays put in the page and comes back with a saved layout", () => {
		const first = dock();
		const panes = createPanes(named);
		panes.attach(first);
		const stop = keepLayout(first, "main");
		act(() => {
			panes.open("a");
			panes.openFile("a", "/w/part.glb");
			panes.openFile("a", "/w/part.glb");
		});
		stop();
		const id = paneId({ kind: "file", agent: "a", path: "/w/part.glb" });
		expect(ids(first)).toEqual(["a", id].sort());
		// Moving the viewer's frame in the page would reload it.
		expect(first.getPanel(id)?.api.renderer).toBe("always");
		expect(first.getPanel("a")?.api.renderer).not.toBe("always");

		const second = dock();
		act(() => restoreLayout(second, "main", () => true));
		expect(second.getPanel(id)?.api.component).toBe(FILE_PANE);
		expect(second.getPanel(id)?.api.renderer).toBe("always");
	});

	it("keeps one pane for a file opened at different lines, and hands each line on", () => {
		const api = dock();
		const panes = createPanes(named);
		panes.attach(api);
		act(() => panes.openFile("a", "/w/main.rs", { line: 40 }));
		const first = revealed("a", "/w/main.rs");
		expect(first?.line).toBe(40);
		act(() => panes.openFile("a", "/w/main.rs", { line: 40 }));
		const again = revealed("a", "/w/main.rs");
		// The same line asked again is a new ask, so it scrolls there again.
		expect(again?.line).toBe(40);
		expect(again?.ask).not.toBe(first?.ask);
		act(() => panes.openFile("a", "/w/main.rs", { line: 7 }));
		expect(revealed("a", "/w/main.rs")?.line).toBe(7);
		expect(ids(api)).toEqual([paneId({ kind: "file", agent: "a", path: "/w/main.rs" })]);
		// Opened without a line, it forgets the old one.
		act(() => panes.openFile("a", "/w/main.rs"));
		expect(revealed("a", "/w/main.rs")).toBeNull();
	});
});

describe("a visual's pane", () => {
	const visual = { html: "<p>Sales</p>", title: "Sales / month", from: "Usage report" };
	const params = { kind: "visual", key: visualKey(visual.html), title: visual.title } as const;

	it("has an ID that gives its key and title back, slashes and all", () => {
		const id = paneId(params);
		expect(paneParams(id)).toEqual(params);
		expect(paneAgent(id)).toBeNull();
	});

	it("opens once, keeps its page out of the layout, and stays put in the page", () => {
		const api = dock();
		const panes = createPanes(named);
		panes.attach(api);
		const stop = keepLayout(api, "main");
		act(() => {
			panes.openVisual(visual);
			panes.openVisual(visual);
		});
		stop();
		const id = paneId(params);
		expect(ids(api)).toEqual([id]);
		expect(api.getPanel(id)?.params).toEqual(params);
		expect(api.getPanel(id)?.api.renderer).toBe("always");
		expect(visualPage(params.key)).toEqual(visual);
		expect(localStorage.getItem(layoutKey("main"))).not.toContain("<p>Sales</p>");
	});

	it("comes back with a saved layout only while its page is here", () => {
		const first = dock();
		const panes = createPanes(named);
		panes.attach(first);
		const stop = keepLayout(first, "main");
		act(() => {
			panes.open("a");
			panes.openVisual(visual);
		});
		stop();
		const id = paneId(params);

		const kept = dock();
		act(() => restoreLayout(kept, "main", pageKept));
		expect(kept.getPanel(id)?.api.component).toBe(VISUAL_PANE);

		// A restart leaves no page behind.
		clearVisualPages();
		const restarted = dock();
		act(() => restoreLayout(restarted, "main", pageKept));
		expect(ids(restarted)).toEqual(["a"]);
	});
});

describe("saving and restoring the layout", () => {
	it("restores what a window saved, without the agents that are gone", () => {
		const first = dock();
		const panes = createPanes(named);
		panes.attach(first);
		const stop = keepLayout(first, "main");
		act(() => {
			panes.open("a");
			panes.open("b", { split: true });
			panes.open("c");
		});
		stop();
		expect(loadLayout("main")).not.toBeNull();

		const second = dock();
		act(() => restoreLayout(second, "main", not("c")));
		expect(ids(second)).toEqual(["a", "b"]);
		expect(second.groups).toHaveLength(2);
	});

	it("starts clean from a layout with panes it doesn't know", () => {
		const first = dock();
		const stop = keepLayout(first, "main");
		act(() => {
			first.addPanel({ id: "x", component: AGENT_PANE, title: "X" });
		});
		stop();
		const saved = JSON.parse(localStorage.getItem(layoutKey("main"))!);
		saved.dock.panels.x.contentComponent = "terminal";
		localStorage.setItem(layoutKey("main"), JSON.stringify(saved));

		const second = dock();
		act(() => restoreLayout(second, "main", () => true));
		expect(second.panels).toHaveLength(0);
		expect(localStorage.getItem(layoutKey("main"))).toBeNull();
	});

	it("starts clean from a layout dockview can't load", () => {
		localStorage.setItem(
			layoutKey("main"),
			JSON.stringify({
				version: 1,
				dock: {
					grid: { root: { type: "branch", data: "nope" }, width: 800, height: 600 },
					panels: { a: { id: "a", contentComponent: AGENT_PANE } },
				},
			})
		);
		const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
		const api = dock();
		act(() => restoreLayout(api, "main", () => true));
		expect(api.panels).toHaveLength(0);
		expect(localStorage.getItem(layoutKey("main"))).toBeNull();
		expect(warn).toHaveBeenCalled();

		const panes = createPanes(named);
		panes.attach(api);
		act(() => panes.open("a"));
		expect(ids(api)).toEqual(["a"]);
	});
});
