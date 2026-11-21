// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import App from "@/App";
import { ServicesProvider } from "@/app/services";
import { mockSource } from "@/data/mock-source";
import { clearKept } from "@maxhq/shared/agent-view/kept";
import type { Agent } from "@maxhq/shared/bindings";
import { MAC_LOCAL_AGENT } from "@maxhq/shared/data/mock-local-models";
import type { DataSource } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

import { AgentView } from "./agent-view";
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

/** The sample source, with the Mac's local agent changed by `over`, and that agent. */
const localAgent = async (over: Partial<Agent> = {}) => {
	const source = mockSource({ now: () => NOW, tick: false });
	const snapshot = source.snapshot;
	const changed: DataSource["snapshot"] = async () => {
		const state = await snapshot();
		return {
			...state,
			entities: state.entities.map((e) =>
				e.entity === "agent" && e.id === MAC_LOCAL_AGENT ? { ...e, ...over } : e
			),
		};
	};
	const found = (await changed()).entities.find(
		(e) => e.entity === "agent" && e.id === MAC_LOCAL_AGENT
	);
	if (found?.entity !== "agent") throw new Error("No local agent in the sample");
	return { source: Object.assign(source, { snapshot: changed }), agent: found };
};

const show = async (over: Partial<Agent> = {}) => {
	const { source, agent } = await localAgent(over);
	render(
		<ServicesProvider source={source}>
			<FixedNow value={NOW}>
				<div style={{ height: 600 }}>
					<AgentView agent={agent} />
				</div>
			</FixedNow>
		</ServicesProvider>
	);
	// The composer comes once the view knows the agent's device.
	await screen.findByRole("textbox", { name: /Message/ });
	return screen.getByRole("region", { name: agent.title });
};

const speedLine = (): HTMLElement | null => document.querySelector("[data-local-speed]");

describe("a local agent's view", () => {
	it("shows Loading model above the composer while its turn waits for the model", async () => {
		const pane = await show({ model: "gpt-oss-20b@mxfp4" });
		const step = await waitFor(() => {
			const found = pane.querySelector<HTMLElement>("[data-loading-model]");
			expect(found).not.toBeNull();
			return found!;
		});
		expect(step).toHaveAttribute("role", "status");
		expect(step).toHaveTextContent("Loading modelgpt-oss-20b");
		const composer = within(pane).getByRole("textbox", { name: /Message/ });
		expect(
			step.compareDocumentPosition(composer) & Node.DOCUMENT_POSITION_FOLLOWING
		).toBeTruthy();
	});

	it("shows the speed line while the turn runs, once its model is in memory", async () => {
		const pane = await show();
		expect(pane.querySelector("[data-loading-model]")).toBeNull();
		const speed = speedLine()!;
		expect(speed).toHaveAttribute("data-local-speed", "live");
		expect(speed.querySelector("[aria-hidden]")).toHaveTextContent(
			"1,180 / 74 tok/s · 31k of 66k"
		);
		expect(within(speed).getByText(/^Reads the prompt at 1,180 tokens\/s/)).toHaveClass(
			"sr-only"
		);
	});

	it("keeps the last turn's speed, dimmed, after it ends", async () => {
		await show({ activity: { state: "done_unread" } });
		const speed = speedLine()!;
		expect(speed).toHaveAttribute("data-local-speed", "last");
		expect(speed).toHaveClass("text-subtle-foreground");
		expect(
			within(speed).getByText(
				"Last turn: Reads the prompt at 1,180 tokens/s · Writes 74 tokens/s · Context 31k of 66k in use"
			)
		).toBeInTheDocument();
	});

	it("never shows Claude's name or mark, and Local's chip in their place", async () => {
		const pane = await show();
		await waitFor(() => expect(pane.querySelector('[data-mark="local"]')).not.toBeNull());
		expect(document.querySelector('[data-mark="claude"]')).toBeNull();
		const header = pane.querySelector("header") ?? pane;
		expect(header.textContent).not.toMatch(/Claude/);
		expect(header.textContent).toMatch(/Local/);
		const footer = pane.querySelector("[data-footer-row]")!;
		expect(footer.textContent).not.toMatch(/Claude/);

		// Its models sit under their server, and Claude is somewhere else to move to.
		const [model] = within(pane).getAllByRole("button", { name: /^Model: / });
		fireEvent.click(model!);
		const menu = await screen.findByRole("menu");
		expect(within(menu).getByText("This Mac")).toBeInTheDocument();
		expect(within(menu).getByText("Move to Claude Max")).toBeInTheDocument();
		expect(within(menu).getAllByText("Fits")).toHaveLength(3);
		expect(within(menu).getByRole("menuitem", { name: "Set up local models…" })).toBeVisible();
	});
});

describe("a local agent's sidebar row", () => {
	it("says Loading model while its turn waits for the model", async () => {
		const { source } = await localAgent({ model: "gpt-oss-20b@mxfp4" });
		render(
			<FixedNow value={NOW}>
				<App source={source} ui={createUiStore()} />
			</FixedNow>
		);
		const nav = screen.getByRole("navigation", { name: "Agents" });
		const title = await within(nav).findByText("Parser edge-case tests");
		const row = title.closest<HTMLElement>("[data-agent-row]")!;
		expect(within(row).getByText("Loading model")).toHaveAttribute("data-loading-model");
	});

	it("shows its usual meta once the model is in memory", async () => {
		const { source } = await localAgent();
		render(
			<FixedNow value={NOW}>
				<App source={source} ui={createUiStore()} />
			</FixedNow>
		);
		const nav = screen.getByRole("navigation", { name: "Agents" });
		const title = await within(nav).findByText("Parser edge-case tests");
		const row = title.closest<HTMLElement>("[data-agent-row]")!;
		expect(within(row).queryByText("Loading model")).toBeNull();
	});
});
