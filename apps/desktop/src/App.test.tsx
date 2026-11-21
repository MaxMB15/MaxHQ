// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { mockSource } from "@/data/mock-source";
import type { DataSource } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

import App from "./App";

const renderApp = (source: DataSource = mockSource({ now: () => NOW, tick: false })) => {
	const ui = createUiStore();
	render(
		<FixedNow value={NOW}>
			<App source={source} ui={ui} />
		</FixedNow>
	);
	return { source, ui };
};

const agentsNav = () => screen.getByRole("navigation", { name: "Agents" });

describe("App", () => {
	it("groups agents by project, each row with both dots", async () => {
		renderApp();
		const nav = agentsNav();
		const maxhq = await within(nav).findByRole("region", { name: "MaxHQ" });
		const groups = within(nav)
			.getAllByRole("region")
			.map((r) => r.getAttribute("aria-label"));
		expect(groups).toEqual([
			"3 agents need you",
			"Pinned",
			"AtlasMaps",
			"Comet",
			"DuskTools",
			"example-api",
			"example-app",
			"MaxHQ",
			"Mosaic",
			"Snoozed",
			"Settled",
		]);

		const row = within(maxhq).getByRole("option", { name: /Release checklist/ });
		expect(within(row).getByRole("img", { name: "Needs input" })).toBeInTheDocument();
		expect(within(row).getByRole("img", { name: "Live" })).toBeInTheDocument();
		// A busy agent stays in its project while the Working shelf is off.
		const busy = within(maxhq).getByRole("option", { name: /M1 observe milestone/ });
		expect(within(busy).getByRole("img", { name: "Sub-agents busy" })).toBeInTheDocument();
	});

	it("nests the agents an agent started and says who started each", async () => {
		renderApp();
		const maxhq = await within(agentsNav()).findByRole("region", { name: "MaxHQ" });
		fireEvent.click(within(maxhq).getByRole("option", { name: /Write the release notes/ }));
		expect(await screen.findByText("Started by Release checklist on Mac")).toBeInTheDocument();
	});

	it("folds more than two started agents into one row, but for the one that needs you", async () => {
		renderApp();
		const maxhq = await within(agentsNav()).findByRole("region", { name: "MaxHQ" });
		const fold = within(maxhq).getByRole("button", { name: "6 started, 1 working" });
		expect(fold).toHaveAttribute("aria-expanded", "false");
		expect(within(maxhq).getByRole("option", { name: /Tag the release/ })).toBeInTheDocument();
		expect(within(maxhq).queryByRole("option", { name: /Write the launch post/ })).toBeNull();

		fireEvent.click(fold);
		expect(fold).toHaveAttribute("aria-expanded", "true");
		expect(
			within(maxhq).getByRole("option", { name: /Write the launch post/ })
		).toBeInTheDocument();
		expect(
			within(maxhq).getByRole("option", { name: /Check the download links/ })
		).toBeInTheDocument();
		// It's a button, so Enter and Space toggle it as a click does.
		expect(fold.tagName).toBe("BUTTON");
		fireEvent.click(fold);
		expect(within(maxhq).queryByRole("option", { name: /Write the launch post/ })).toBeNull();
	});

	it("opens an agent with sub-agents in the main area and the side panel", async () => {
		renderApp();
		fireEvent.click(await screen.findByRole("option", { name: /M1 observe milestone/ }));

		expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("M1 observe milestone");
		const panel = screen.getByRole("complementary", {
			name: "Sub-agents of M1 observe milestone",
		});
		expect(within(panel).getByText("Claude Max on Mac, 7 sub-agents")).toBeInTheDocument();
		expect(within(panel).getByRole("button", { name: /W4 client crate/ })).toHaveTextContent(
			"Done"
		);

		fireEvent.click(within(panel).getByRole("button", { name: "Close sub-agents" }));
		expect(screen.queryByRole("complementary")).not.toBeInTheDocument();
	});

	it("lists what needs you and past notifications from the bell", async () => {
		const { ui } = renderApp();
		const bell = await screen.findByRole("button", { name: /agents need you$/ });
		fireEvent.click(bell);

		const popup = await screen.findByRole("dialog", { name: "Notifications" });
		const now = within(popup).getByRole("region", { name: "Now" });
		const earlier = within(popup).getByRole("region", { name: "Earlier" });
		expect(bell).toHaveAccessibleName(
			`${within(now).getAllByRole("listitem").length} agents need you`
		);
		expect(within(now).getByRole("button", { name: /Release checklist/ })).toHaveTextContent(
			"Stays until you answer it"
		);
		const [gone] = await within(earlier).findAllByRole("button", {
			name: /Schema migration dry run/,
		});
		expect(gone).toHaveTextContent("This agent is gone");
		// The first page holds 20 notices, and the newest two are the prompts
		// Now shows, so Earlier leaves them out.
		expect(within(earlier).getAllByRole("listitem")).toHaveLength(18);
		fireEvent.click(within(popup).getByRole("button", { name: "Show more" }));
		await waitFor(() => expect(within(earlier).getAllByRole("listitem")).toHaveLength(25));

		fireEvent.click(within(now).getByRole("button", { name: /Release checklist/ }));
		expect(ui.getState().selectedAgent).toBe("agent-release");
		await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
	});

	it("shows plan usage with the at-limit account in the popover", async () => {
		renderApp();
		const meter = await screen.findByRole("button", { name: /^Plan usage\. Claude/ });
		expect(meter).toHaveAccessibleName(/Codex at its limit/);
		fireEvent.click(meter);

		const popup = await screen.findByRole("dialog");
		const codex = within(popup).getByRole("region", { name: "Codex Pro" });
		expect(codex).toHaveTextContent("At limit, 1 agent waiting");
		expect(within(popup).getByRole("region", { name: "Claude Max" })).toHaveTextContent(
			"5 hours"
		);
	});

	it("lists devices by network and shows where the connection stops", async () => {
		const { source } = renderApp();
		fireEvent.click(screen.getByRole("button", { name: "Devices" }));
		const nav = screen.getByRole("navigation", { name: "Devices" });
		const tailnet = await within(nav).findByRole("region", { name: "Tailnet tail1234" });
		expect(within(nav).getByRole("region", { name: "This Mac" })).toBeInTheDocument();

		fireEvent.click(within(tailnet).getByRole("button", { name: /^.*devbox.*agents$/ }));
		expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("devbox");
		expect(screen.getByRole("button", { name: "Copy SSH command" })).toBeInTheDocument();

		act(() => source.mock?.setScenario("peer_offline"));
		expect(await within(tailnet).findByText("Offline")).toBeInTheDocument();
		const checks = screen.getByRole("region", { name: "Connection checks" });
		expect(within(checks).getAllByRole("img", { name: "Failed" })).toHaveLength(1);
		expect(within(checks).getAllByRole("img", { name: "Not run" })).toHaveLength(3);
		expect(screen.getByRole("heading", { name: "Agents last seen here" })).toBeInTheDocument();
	});

	it("says it shows sample data in a plain browser", async () => {
		renderApp();
		expect(await screen.findByText(/Browser preview with sample data/)).toBeInTheDocument();
		expect(screen.getByRole("combobox", { name: "Sample setup" })).toHaveValue("connected");
	});

	it("says when the backend doesn't answer", async () => {
		const base = mockSource({ now: () => NOW, tick: false });
		const broken: DataSource = {
			...base,
			kind: "tauri",
			mock: undefined,
			appInfo: () => Promise.reject(new Error("no handler")),
		};
		renderApp(broken);
		expect(await screen.findByText(/backend didn't answer/)).toHaveTextContent("no handler");
		expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
	});
});
