// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Entity, RunKey } from "../bindings";
import { createUiStore, slideLead, spotOf } from "../ui/store";
import { applySnapshot } from "../view/replica";
import { createViewStore } from "../view/store";
import { watchHandOver } from "./hand-over";

const RUN: RunKey = { device: "d1", project: "p1", workflow: "pipeline", run: "t-1" };

const runEntity = (orchestrators: string[]): Entity => ({
	entity: "workflow_run",
	id: "wfr-t-1",
	key: RUN,
	label: "Run",
	progress: { done: 0, total: 4 },
	current: [],
	needs_you: false,
	orchestrators,
	last_activity: null,
	done: false,
});

describe("the slide-out", () => {
	it("opens a lead's chip on the Workflow tab", () => {
		const ui = createUiStore();
		ui.getState().openRun(RUN, "lead");
		expect(ui.getState().slideOut).toEqual({ lead: "lead", run: RUN, tab: "workflow" });
		expect(ui.getState().selectedAgent).toBe("lead");
	});

	it("keeps each lead's last tab for the next time it opens", () => {
		const ui = createUiStore();
		ui.getState().selectAgent("lead", true);
		expect(ui.getState().slideOut).toEqual({ lead: "lead", run: null, tab: "agents" });
		ui.getState().setSlideTab("workflow");
		ui.getState().selectAgent("other", true);
		expect(ui.getState().slideOut?.tab).toBe("agents");
		ui.getState().selectAgent("lead", true);
		expect(ui.getState().slideOut?.tab).toBe("workflow");
		ui.getState().selectAgent("solo", false);
		expect(ui.getState().slideOut).toBeNull();
	});

	it("opens a lead on the tab it's given until it keeps one", () => {
		const ui = createUiStore();
		ui.getState().selectAgent("lead", true, "workflow");
		expect(ui.getState().slideOut).toEqual({ lead: "lead", run: null, tab: "workflow" });
		ui.getState().setSlideTab("agents");
		ui.getState().selectAgent("lead", true, "workflow");
		expect(ui.getState().slideOut?.tab).toBe("agents");
	});

	it("points out a sub-agent's row on the Sub-agents tab, until you pick a row or a tab", () => {
		const ui = createUiStore({
			selectedAgent: "other",
			slideTabs: new Map([["lead", "workflow"]]),
		});
		ui.getState().showSubAgent("lead", "w2");
		expect(ui.getState().slideOut).toEqual({
			lead: "lead",
			run: null,
			tab: "agents",
			spot: "w2",
		});
		expect(ui.getState().selectedAgent).toBe("lead");
		expect(spotOf(ui.getState(), "lead")).toBe("w2");
		expect(spotOf(ui.getState(), "other")).toBeNull();
		// The lead keeps the tab you last picked for it.
		expect(ui.getState().slideTabs.get("lead")).toBe("workflow");

		ui.getState().selectSubAgent("w3");
		expect(ui.getState().slideOut).toEqual({ lead: "lead", run: null, tab: "agents" });

		ui.getState().showSubAgent("lead", "w2");
		ui.getState().setSlideTab("workflow");
		ui.getState().setSlideTab("agents");
		expect(spotOf(ui.getState(), "lead")).toBeNull();
	});

	it("opens an unbound run in run mode and hands it to its lead once one binds it", () => {
		const view = createViewStore();
		const ui = createUiStore({ selectedAgent: "someone" });
		const stop = watchHandOver(view, ui);
		view.setState({ view: applySnapshot({ seq: 1, entities: [runEntity([])] }) });
		ui.getState().openRun(RUN);
		expect(ui.getState().slideOut).toEqual({ lead: null, run: RUN, tab: "workflow" });
		expect(ui.getState().selectedAgent).toBe("someone");
		expect(slideLead(ui.getState())).toBeNull();

		view.setState({ view: applySnapshot({ seq: 2, entities: [runEntity(["lead"])] }) });
		expect(ui.getState().slideOut).toEqual({ lead: "lead", run: RUN, tab: "workflow" });
		ui.getState().selectAgent("lead", true);
		expect(ui.getState().slideOut?.tab).toBe("workflow");
		stop();
	});

	it("leaves run mode when you select an agent", () => {
		const ui = createUiStore();
		ui.getState().openRun(RUN);
		ui.getState().selectAgent("solo", false);
		expect(ui.getState().slideOut).toBeNull();
		ui.getState().openRun(RUN);
		ui.getState().selectAgent("lead", true);
		expect(ui.getState().slideOut).toEqual({ lead: "lead", run: null, tab: "agents" });
	});

	it("leaves a lead's slide-out alone when another run binds", () => {
		const view = createViewStore();
		const ui = createUiStore();
		const stop = watchHandOver(view, ui);
		ui.getState().openRun(RUN, "mine");
		view.setState({ view: applySnapshot({ seq: 1, entities: [runEntity(["theirs"])] }) });
		expect(slideLead(ui.getState())).toBe("mine");
		ui.getState().closeSubAgents();
		expect(ui.getState().slideOut).toBeNull();
		stop();
	});
});
