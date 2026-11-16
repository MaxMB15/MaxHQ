// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import { clearKept } from "@maxhq/shared/agent-view/kept";
import { keepSubAgent } from "@maxhq/shared/agent-view/finished";
import { type AgentId, type Delta, FINISHED_SUB_AGENTS_SINCE } from "@maxhq/shared/bindings";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

import { clearSnapshots } from "./timeline-list";

// jsdom lays nothing out, and the list draws no rows into a window of no
// height. Give every element a size.
beforeAll(() => {
	vi.spyOn(Element.prototype, "clientHeight", "get").mockReturnValue(600);
	vi.spyOn(Element.prototype, "clientWidth", "get").mockReturnValue(800);
});
afterAll(() => vi.restoreAllMocks());
afterEach(() => {
	clearKept();
	clearSnapshots();
});

/**
 * The sample source, with every device on a maxd that reads finished
 * sub-agents, and a way to end an agent the way maxd stops listing it.
 */
const sampleSource = () => {
	const source = mockSource({ now: () => NOW, tick: false });
	const ended = new Set<AgentId>();
	let seq = 0;
	let deliver: (delta: Delta) => void = () => {};
	const { snapshot, subscribe } = source;
	Object.assign(source, {
		snapshot: async () => {
			const state = await snapshot();
			seq = state.seq;
			return {
				...state,
				entities: state.entities
					.filter((e) => !(e.entity === "agent" && ended.has(e.id)))
					.map((e) =>
						e.entity === "device_link"
							? { ...e, protocol: FINISHED_SUB_AGENTS_SINCE }
							: e
					),
			};
		},
		subscribe: (onDelta: (delta: Delta) => void) => {
			deliver = onDelta;
			return subscribe((delta) => {
				seq = delta.seq;
				onDelta(delta);
			});
		},
	});
	const end = (agent: AgentId) => {
		ended.add(agent);
		seq += 1;
		act(() =>
			deliver({ seq, changes: [{ op: "remove", key: { entity: "agent", id: agent } }] })
		);
	};
	return { source, end };
};

const renderApp = (source = sampleSource().source) =>
	render(
		<FixedNow value={NOW}>
			<App source={source} ui={createUiStore()} />
		</FixedNow>
	);

const agents = () => screen.getByRole("navigation", { name: "Agents" });
const LEAD = "Clean up old flags";

/** Opens the lead from the sidebar and returns its slide-out. */
const openLead = async () => {
	fireEvent.click(await within(agents()).findByRole("option", { name: new RegExp(LEAD) }));
	return screen.getByRole("complementary", { name: `Sub-agents of ${LEAD}` });
};

/** The pane a heading titles. */
const paneOf = async (title: string) =>
	(await screen.findByRole("heading", { level: 1, name: title })).closest("section")!;

describe("a finished sub-agent", () => {
	it("opens from its lead's Workflow list as its history, read only", async () => {
		renderApp();
		const panel = await openLead();
		fireEvent.click(within(panel).getByRole("tab", { name: "Workflow" }));
		const nodes = await within(panel).findByRole("list", { name: "Nodes" });
		fireEvent.click(within(nodes).getByRole("button", { name: /^Flags in the docs, Done/ }));

		const pane = await paneOf("Flags in the docs");
		expect(within(pane).getByText("Finished, read only")).toBeInTheDocument();
		expect(within(pane).queryByText(/isn't here/)).not.toBeInTheDocument();
		expect(within(pane).queryByRole("textbox")).not.toBeInTheDocument();
		const note = within(pane).getByRole("region", { name: "Sub-agent finished" });
		expect(note).toHaveTextContent("This sub-agent finished.");
		// Its history, from the lead's session.
		expect(await within(pane).findByText("Everything is committed.")).toBeInTheDocument();
		// The slide-out stays on the lead.
		expect(
			screen.getByRole("complementary", { name: `Sub-agents of ${LEAD}` })
		).toBeInTheDocument();

		fireEvent.click(within(note).getByRole("button", { name: "Open lead" }));
		expect(await paneOf(LEAD)).toBeInTheDocument();
	});

	it("leaves the sidebar and the Sub-agents tab as they were", async () => {
		renderApp();
		const row = await within(agents()).findByRole("option", { name: new RegExp(LEAD) });
		expect(row).toHaveTextContent("1 sub");
		expect(
			within(agents()).queryByRole("option", { name: /Flags in the docs/ })
		).not.toBeInTheDocument();

		const panel = await openLead();
		expect(within(panel).getByRole("tab", { name: "Sub-agents" })).toHaveAttribute(
			"aria-selected",
			"true"
		);
		expect(
			within(panel).getByRole("button", { name: /Flags in the web app/ })
		).toBeInTheDocument();
		expect(
			within(panel).queryByRole("button", { name: /Flags in the docs/ })
		).not.toBeInTheDocument();
	});

	it("keeps a tab open on it, now read only, once it finishes", async () => {
		const { source, end } = sampleSource();
		renderApp(source);
		const panel = await openLead();
		fireEvent.click(within(panel).getByRole("button", { name: /Flags in the web app/ }));
		const live = await paneOf("Flags in the web app");
		expect(within(live).queryByText("Finished, read only")).not.toBeInTheDocument();

		end("agent-ex-flags-web");
		const pane = await paneOf("Flags in the web app");
		expect(within(pane).getByText("Finished, read only")).toBeInTheDocument();
		expect(within(pane).queryByRole("textbox")).not.toBeInTheDocument();
		expect(
			within(pane).getByRole("region", { name: "Sub-agent finished" })
		).toBeInTheDocument();
	});

	it("says plainly that it finished once its lead is gone too", async () => {
		const source = sampleSource().source;
		let focus: (agent: AgentId) => void = () => {};
		Object.assign(source, {
			onFocusAgent: async (onFocus: typeof focus) => {
				focus = onFocus;
				return () => {};
			},
		});
		keepSubAgent("sub-gone", "lead-gone", "Gone with its lead");
		renderApp(source);
		await within(agents()).findByRole("option", { name: new RegExp(LEAD) });
		act(() => focus("sub-gone"));
		expect(
			await screen.findByText("This sub-agent finished, and its lead is no longer here.")
		).toBeInTheDocument();
	});
});
