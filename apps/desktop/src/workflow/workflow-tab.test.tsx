// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { NodeStatus, RunSnapshot } from "@maxhq/shared/bindings";
import { mockRunEntity, mockRunSnapshot, RUN_LEADS } from "@maxhq/shared/data/mock-workflow";
import { NOW } from "@maxhq/shared/test/build";
import { GLYPHS } from "@maxhq/shared/workflow/glyphs";
import { applyRunUpdate, EMPTY_RUN, type RunState } from "@maxhq/shared/workflow/store";

import * as glyph from "./glyph";
import { NodeGlyph } from "./glyph";
import { type WorkflowTabProps, WorkflowTab } from "./workflow-tab";

vi.mock("./glyph", async (load) => {
	const real = await load<typeof import("./glyph")>();
	return { ...real, NodeGlyph: vi.fn(real.NodeGlyph) };
});

// Busy in its analysis gates, waiting on approval with the mockup aside,
// and a run with no event log.
const FILTERS = "t-20261101-example-filters";
const SETTINGS = "t-20261101-example-settings";
const DATES = "t-20261101-example-dates";

/** The props a run's store gives the tab. */
const fromState = (state: RunState) => ({
	shape: state.shape,
	nodes: state.nodes,
	loops: state.loops,
	header: state.header,
	load: { status: state.status, notice: state.notice, error: state.error },
});

const loaded = (snapshot: RunSnapshot) =>
	applyRunUpdate(EMPTY_RUN, { kind: "reset", run: snapshot })!;

const tab = (run = FILTERS, over: Partial<WorkflowTabProps> = {}) => {
	const entity = mockRunEntity(NOW, run);
	const props: WorkflowTabProps = {
		runs: [entity],
		run: entity,
		...fromState(loaded(mockRunSnapshot(NOW, run))),
		selected: null,
		named: true,
		onPickRun: vi.fn(),
		onOpenAgent: vi.fn(),
		...over,
	};
	return { props, ...render(<WorkflowTab {...props} />) };
};

const nodes = () => screen.getByRole("list", { name: "Nodes" });
const row = (label: string) =>
	within(nodes()).getByRole("button", { name: new RegExp(`^${label}, `) });

describe("WorkflowTab", () => {
	it("gives every status a glyph a screen reader can name", () => {
		for (const status of Object.keys(GLYPHS) as NodeStatus[]) {
			const { unmount } = render(<NodeGlyph status={status} provisional={false} />);
			expect(screen.getByRole("img", { name: GLYPHS[status].label })).toBeInTheDocument();
			unmount();
		}
		render(<NodeGlyph status="done" provisional />);
		expect(screen.getByRole("img", { name: "Done, not yet certain" })).toBeInTheDocument();
	});

	it("shows a row per node, with omitted nodes left out and a folded group as one row", () => {
		tab();
		expect(within(nodes()).getAllByRole("listitem")).toHaveLength(9);
		expect(screen.queryByText("Mockup")).not.toBeInTheDocument();
		expect(
			within(nodes()).getByRole("button", { name: "Analysis, 2 of 5 nodes done" })
		).toHaveAttribute("aria-expanded", "false");
	});

	it("puts a node the workflow sets aside on the second lane", () => {
		tab(SETTINGS);
		expect(row("Mockup")).toBeInTheDocument();
		expect(within(nodes()).getAllByRole("listitem")).toHaveLength(10);
	});

	it("opens a group to show its members", () => {
		tab();
		fireEvent.click(within(nodes()).getByRole("button", { name: /^Analysis/ }));
		expect(row("Testing")).toBeInTheDocument();
		expect(row("Performance")).toBeInTheDocument();
	});

	it("opens a node's agent on click, and splits with Command", () => {
		const { props } = tab();
		fireEvent.click(within(nodes()).getByRole("button", { name: /^Analysis/ }));
		expect(row("Testing")).toHaveAccessibleName("Testing, Running, opens its agent");
		fireEvent.click(row("Testing"));
		expect(props.onOpenAgent).toHaveBeenLastCalledWith("agent-ex-testing", false);
		fireEvent.click(row("Testing"), { ctrlKey: true });
		expect(props.onOpenAgent).toHaveBeenLastCalledWith("agent-ex-testing", true);
	});

	it("opens a node with no agent on its orchestrator while it's under way", () => {
		const { props } = tab(SETTINGS);
		fireEvent.click(row("Approve"));
		expect(props.onOpenAgent).toHaveBeenLastCalledWith(RUN_LEADS.approve, false);
	});

	it("opens the popover with the note for a settled node with no agent", async () => {
		const { props } = tab();
		fireEvent.click(row("Approve"));
		expect(await screen.findByText("Not ui work, so nothing to approve")).toBeInTheDocument();
		expect(props.onOpenAgent).not.toHaveBeenCalled();
	});

	it("lists every attempt in the popover, with a way to open each one's agent", async () => {
		const { props } = tab(SETTINGS);
		fireEvent.click(screen.getByRole("button", { name: "More about Mockup" }));
		const attempts = await screen.findByRole("list", { name: "Attempts" });
		expect(within(attempts).getByText("Try 1")).toBeInTheDocument();
		expect(within(attempts).getByText("Try 2")).toBeInTheDocument();
		fireEvent.click(within(attempts).getByRole("button", { name: "Open" }));
		expect(props.onOpenAgent).toHaveBeenLastCalledWith("agent-ex-mockup", false);
	});

	it("shows the node's detail lists in the popover", async () => {
		const onDetail = vi.fn(async () => ({
			lists: [{ label: "Files read", items: ["src/search/filters.ts"] }],
		}));
		tab(FILTERS, { onDetail });
		fireEvent.click(screen.getByRole("button", { name: "More about Research" }));
		expect(await screen.findByText("src/search/filters.ts")).toBeInTheDocument();
		expect(onDetail).toHaveBeenCalledWith("research");
	});

	it("shows the selected agent's node as selected", () => {
		tab(SETTINGS, { selected: "agent-ex-mockup" });
		expect(row("Mockup").closest("li")).toHaveAttribute("aria-current", "true");
		expect(row("Research").closest("li")).not.toHaveAttribute("aria-current");
	});

	it("names a loop's count to a screen reader", () => {
		tab(SETTINGS);
		expect(screen.getByRole("img", { name: "approve went round 1/3" })).toBeInTheDocument();
	});

	it("shows the run's progress and its notice", () => {
		const { props } = tab(DATES);
		const { done, total } = props.header!.progress;
		expect(screen.getByRole("img", { name: `${done} of ${total} done` })).toBeInTheDocument();
		expect(screen.getByRole("status")).toHaveTextContent("This run has no event log");
	});

	it("shows the descriptor's header lines", () => {
		tab("t-20261101-example-sync");
		expect(screen.getByText("retries 10/12")).toBeInTheDocument();
	});

	it("says it's loading until the run's state arrives, and why it couldn't open", () => {
		const { props, rerender } = tab(FILTERS, fromState(EMPTY_RUN));
		expect(screen.getByText("Loading the run")).toBeInTheDocument();
		expect(screen.queryByRole("list", { name: "Nodes" })).not.toBeInTheDocument();
		rerender(
			<WorkflowTab
				{...props}
				load={{ status: "failed", notice: null, error: "No such run" }}
			/>
		);
		expect(screen.getByRole("alert")).toHaveTextContent("Couldn't open the run: No such run");
	});

	it("offers a picker only when the lead has more than one run", async () => {
		const second = mockRunEntity(NOW, SETTINGS);
		const { props, rerender } = tab();
		expect(screen.queryByRole("button", { name: /^Pick a run/ })).not.toBeInTheDocument();
		rerender(<WorkflowTab {...props} runs={[props.run, second]} />);
		fireEvent.click(screen.getByRole("button", { name: "Pick a run, 1 of 2" }));
		fireEvent.click(await screen.findByRole("menuitem", { name: /Settings page redesign/ }));
		expect(props.onPickRun).toHaveBeenCalledWith(second.id);
	});

	it("offers the canvas only when there's a pane for it", () => {
		const onOpenCanvas = vi.fn();
		const { props, rerender } = tab();
		expect(screen.queryByRole("button", { name: "Open canvas" })).not.toBeInTheDocument();
		rerender(<WorkflowTab {...props} onOpenCanvas={onOpenCanvas} />);
		fireEvent.click(screen.getByRole("button", { name: "Open canvas" }));
		expect(onOpenCanvas).toHaveBeenCalled();
	});

	it("renders only the row whose node changed", () => {
		// The run with no descriptor has plain rows, so each glyph drawn is one row rendering.
		const snapshot = mockRunSnapshot(NOW, RUN_LEADS.agents);
		const start = loaded(snapshot);
		const { props, rerender } = tab(RUN_LEADS.agents, fromState(start));
		const drawn = vi.mocked(glyph.NodeGlyph);
		drawn.mockClear();
		const docs = { ...start.nodes.get("agent-ex-flags-docs")!, status: "failed" as const };
		const next = applyRunUpdate(start, {
			kind: "patch",
			run: { key: snapshot.key, rev: 2, nodes: [docs], loops: null, header: null },
		})!;
		rerender(<WorkflowTab {...props} {...fromState(next)} />);
		expect(drawn).toHaveBeenCalledTimes(1);
		expect(drawn.mock.calls[0]![0]).toMatchObject({ status: "failed" });
	});
});
