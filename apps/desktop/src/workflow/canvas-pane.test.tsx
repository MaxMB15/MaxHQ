// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import type { WorkflowUpdate } from "@maxhq/shared/bindings";
import { mockRunSnapshot, RUN_LEADS } from "@maxhq/shared/data/mock-workflow";
import type { DataSource } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";
import { nodeAgent } from "@maxhq/shared/workflow/layout";
import { runName } from "@maxhq/shared/workflow/select";

import * as glyph from "./glyph";
import { laidOut, layOut } from "./elk";

vi.mock("./glyph", async (load) => {
	const real = await load<typeof import("./glyph")>();
	return { ...real, NodeGlyph: vi.fn(real.NodeGlyph) };
});

const FILTERS = mockRunSnapshot(NOW, "t-20261101-example-filters");
const SETTINGS = mockRunSnapshot(NOW, "t-20261101-example-settings");

// The mock source, keeping each open run's listener so a test can patch it.
const renderApp = () => {
	const source = mockSource({ now: () => NOW, tick: false });
	const listeners = new Map<string, (update: WorkflowUpdate) => void>();
	const watched: DataSource = {
		...source,
		openWorkflow: (run, onUpdate) => {
			listeners.set(runName(run), onUpdate);
			return source.openWorkflow(run, onUpdate);
		},
	};
	const ui = createUiStore();
	render(
		<FixedNow value={NOW}>
			<App source={watched} ui={ui} />
		</FixedNow>
	);
	return { ui, listeners };
};

const agents = () => screen.getByRole("navigation", { name: "Agents" });

/** Opens a lead's Workflow tab, then its canvas, and closes the slide-out. */
const openCanvas = async (lead: string) => {
	fireEvent.click(await within(agents()).findByRole("option", { name: new RegExp(lead) }));
	const panel = screen.getByRole("complementary", { name: `Sub-agents of ${lead}` });
	fireEvent.click(within(panel).getByRole("tab", { name: "Workflow" }));
	fireEvent.click(await within(panel).findByRole("button", { name: "Open canvas" }));
	fireEvent.click(within(panel).getByRole("button", { name: "Close sub-agents" }));
	return screen.findByRole("application", { name: "Workflow canvas" });
};

const card = (canvas: HTMLElement, label: string) =>
	within(canvas).getByRole("button", { name: new RegExp(`^${label}, `) });

describe("the canvas pane", () => {
	it("opens from the Workflow tab with a card per node, omitted ones included", async () => {
		const { ui } = renderApp();
		const canvas = await openCanvas("Search filters");
		// Focusing the canvas leaves the selection on the lead.
		expect(ui.getState().selectedAgent).toBe(RUN_LEADS.analysis);
		expect(card(canvas, "Research")).toHaveAccessibleName(/^Research, Done/);
		const cards = within(canvas).getAllByRole("button", { name: /, [A-Z]/ });
		expect(cards.length).toBeGreaterThanOrEqual(FILTERS.shape.nodes.length);
		expect(
			within(canvas).getByRole("group", { name: /^Analysis, 2 of 5 done/ })
		).toBeInTheDocument();
		// React Flow's attribution stays.
		expect(
			within(canvas).getByRole("link", { name: "React Flow attribution" })
		).toBeInTheDocument();
	});

	it("draws one card again when one node changes", async () => {
		const { listeners } = renderApp();
		const canvas = await openCanvas("Search filters");
		const draw = vi.mocked(glyph.NodeGlyph);
		draw.mockClear();
		const verify = FILTERS.nodes.find((n) => n.id === "verify")!;
		act(() =>
			listeners.get(runName(FILTERS.key))!({
				kind: "patch",
				run: {
					key: FILTERS.key,
					rev: FILTERS.rev + 1,
					nodes: [{ ...verify, status: "busy" }],
					loops: null,
					header: null,
				},
			})
		);
		expect(card(canvas, "Verify")).toHaveAccessibleName(/^Verify, Running/);
		const drawn = new Set(draw.mock.calls.map(([props]) => props.status));
		expect(drawn).toEqual(new Set(["busy"]));
	});

	it("opens a card's agent, the lead's own sub-agents in place", async () => {
		const { ui } = renderApp();
		const canvas = await openCanvas("Search filters");
		const node = FILTERS.nodes.find((n) => nodeAgent(n, null) !== null)!;
		const label = FILTERS.shape.nodes.find((n) => n.id === node.id)!.label;
		fireEvent.click(card(canvas, label));
		expect(ui.getState().selectedAgent).toBe(nodeAgent(node, null));
	});

	it("shows the loops that ran, and the ones that could with the switch", async () => {
		renderApp();
		const canvas = await openCanvas("Settings page redesign");
		expect(
			within(canvas).getByRole("img", { name: "approve went round 1/3" })
		).toBeInTheDocument();
		expect(
			within(canvas).queryByRole("img", { name: /can go back here/ })
		).not.toBeInTheDocument();
		fireEvent.click(screen.getByRole("switch", { name: "Show possible loops" }));
		expect(
			within(canvas).getByRole("img", { name: "review can go back here" })
		).toBeInTheDocument();
	});

	it("switches to a timeline, hatching the wait from when it began", async () => {
		renderApp();
		await openCanvas("Settings page redesign");
		fireEvent.click(screen.getByRole("radio", { name: "Timeline" }));
		const chart = screen.getByRole("list", { name: "Attempts over time" });
		const omitted = SETTINGS.nodes.filter((n) => n.status === "omitted").length;
		expect(within(chart).getAllByRole("listitem")).toHaveLength(
			SETTINGS.nodes.length - omitted
		);
		const approve = within(chart).getByRole("listitem", { name: "Approve" });
		expect(approve.querySelector('rect[fill="url(#timeline-wait)"]')).not.toBeNull();
		expect(screen.queryByRole("note")).toBeNull();
	});
});

describe("the layout", () => {
	it("is worked out once per shape, as a column under tests", async () => {
		const first = laidOut(FILTERS.shape);
		expect(first).toBeDefined();
		expect(laidOut({ ...FILTERS.shape })).toBe(first);
		await expect(layOut(FILTERS.shape)).resolves.toBe(first);
	});
});
