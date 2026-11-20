// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import type { DataSource } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

const renderApp = () => {
	const source = mockSource({ now: () => NOW, tick: false });
	render(
		<FixedNow value={NOW}>
			<App source={source} ui={createUiStore()} />
		</FixedNow>
	);
	return source;
};

const openAgent = async (title: RegExp) => {
	const nav = screen.getByRole("navigation", { name: "Agents" });
	const maxhq = await within(nav).findByRole("region", { name: "MaxHQ" });
	fireEvent.click(within(maxhq).getByRole("option", { name: title }));
	await screen.findByRole("heading", { level: 1, name: title });
};

const held = async (source: DataSource, agent: string) => {
	const listed = await source.listPermissions(null);
	return listed.status === "ok"
		? listed.data.find((p) => p.holder.kind === "agent" && p.holder.agent === agent)
		: undefined;
};

const openAgentTools = async () => {
	fireEvent.click(await screen.findByRole("button", { name: "Settings" }));
	const nav = await screen.findByRole("navigation", { name: "Settings pages" });
	fireEvent.click(
		within(within(nav).getByRole("group", { name: "App" })).getByRole("button", {
			name: "Agent tools",
		})
	);
	return within(await screen.findByRole("region", { name: "Access keys" }));
};

describe("allowing an agent to run agents", () => {
	it("allows an agent within the devices and vendors picked", async () => {
		const source = renderApp();
		await openAgent(/Write the release notes/);
		expect(screen.queryByRole("button", { name: "Can run agents" })).toBeNull();

		fireEvent.click(screen.getByRole("button", { name: "More" }));
		fireEvent.click(await screen.findByRole("menuitem", { name: /Allow to run agents/ }));
		const sheet = within(await screen.findByRole("dialog", { name: "Allow to run agents" }));
		expect(sheet.getByRole("checkbox", { name: "Claude" })).toBeChecked();
		fireEvent.click(sheet.getByRole("checkbox", { name: "Claude" }));
		fireEvent.change(sheet.getByLabelText("Agents at once"), { target: { value: "2" } });
		fireEvent.click(sheet.getByRole("button", { name: "Allow" }));

		expect(await screen.findByRole("button", { name: "Can run agents" })).toBeInTheDocument();
		const permission = await held(source, "agent-release-notes");
		expect(permission).toMatchObject({
			holder: { kind: "agent", agent: "agent-release-notes" },
			vendors: ["codex"],
			accounts: null,
			places: null,
			depth: 3,
			limits: { max_running: 2, max_started: null },
		});
	});

	it("leases no test machine unless you tick its kind", async () => {
		const source = renderApp();
		await openAgent(/Write the release notes/);
		fireEvent.click(screen.getByRole("button", { name: "More" }));
		fireEvent.click(await screen.findByRole("menuitem", { name: /Allow to run agents/ }));
		const sheet = within(await screen.findByRole("dialog", { name: "Allow to run agents" }));
		const kinds = within(sheet.getByRole("group", { name: "Test hosts" }));
		expect(kinds.getAllByRole("checkbox").map((c) => c.getAttribute("aria-checked"))).toEqual([
			"false",
			"false",
			"false",
			"false",
		]);
		fireEvent.click(kinds.getByRole("checkbox", { name: "Android emulator" }));
		fireEvent.click(sheet.getByRole("button", { name: "Allow" }));

		await screen.findByRole("button", { name: "Can run agents" });
		expect(await held(source, "agent-release-notes")).toMatchObject({
			test_kinds: ["android_emulator"],
		});
	});

	it("asks before an agent in a container reaches outside it, unless you pick another", async () => {
		const source = renderApp();
		await openAgent(/Write the release notes/);
		fireEvent.click(screen.getByRole("button", { name: "More" }));
		fireEvent.click(await screen.findByRole("menuitem", { name: /Allow to run agents/ }));
		const sheet = within(await screen.findByRole("dialog", { name: "Allow to run agents" }));
		const containers = within(sheet.getByRole("group", { name: "Containers" }));
		const pressed = () =>
			containers.getAllByRole("button").map((b) => b.getAttribute("aria-pressed"));
		expect(pressed()).toEqual(["true", "false", "false"]);
		fireEvent.click(containers.getByRole("button", { name: "Never" }));
		expect(pressed()).toEqual(["false", "false", "true"]);
		fireEvent.click(sheet.getByRole("button", { name: "Allow" }));

		await screen.findByRole("button", { name: "Can run agents" });
		expect(await held(source, "agent-release-notes")).toMatchObject({
			leave_container: "never",
		});
	});

	it("says why a sheet can't be saved", async () => {
		renderApp();
		await openAgent(/Write the release notes/);
		fireEvent.click(screen.getByRole("button", { name: "More" }));
		fireEvent.click(await screen.findByRole("menuitem", { name: /Allow to run agents/ }));
		const sheet = within(await screen.findByRole("dialog", { name: "Allow to run agents" }));
		fireEvent.click(sheet.getByRole("checkbox", { name: "Any account" }));
		fireEvent.click(sheet.getByRole("button", { name: "Allow" }));
		expect(sheet.getByRole("alert")).toHaveTextContent(
			"Pick at least one account, or allow any."
		);
	});

	it("revokes the permission an agent holds", async () => {
		const source = renderApp();
		await openAgent(/Release checklist/);
		fireEvent.click(await screen.findByRole("button", { name: "Can run agents" }));
		const sheet = within(await screen.findByRole("dialog", { name: "Allow to run agents" }));
		fireEvent.click(sheet.getByRole("button", { name: "Revoke" }));
		fireEvent.click(sheet.getByRole("button", { name: "Revoke" }));

		await waitFor(() =>
			expect(screen.queryByRole("button", { name: "Can run agents" })).toBeNull()
		);
		expect(await held(source, "agent-release")).toBeUndefined();
	});
});

describe("the agent tools settings", () => {
	it("lists each key's file and copies the command that uses it", async () => {
		const writeText = vi.fn(() => Promise.resolve());
		Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
		renderApp();
		const keys = await openAgentTools();
		expect(
			await keys.findByText("/Users/dev/Library/Application Support/MaxHQ/keys/raycast.key")
		).toBeInTheDocument();
		const raycast = keys.getByText("raycast").closest("div")!.parentElement!;
		fireEvent.click(within(raycast).getByRole("button", { name: "Copy MCP command" }));
		expect(writeText).toHaveBeenCalledWith(
			"/Users/dev/.local/bin/maxd mcp --key-file '/Users/dev/Library/Application Support/MaxHQ/keys/raycast.key'"
		);
	});

	it("makes a key and shows where its file is", async () => {
		const source = renderApp();
		const keys = await openAgentTools();
		await keys.findByText("slack-bot");
		fireEvent.change(keys.getByRole("textbox", { name: "Key name" }), {
			target: { value: "ci bot" },
		});
		fireEvent.click(keys.getByRole("button", { name: "Make key" }));
		expect(await keys.findByText(/^Use only letters/)).toBeInTheDocument();

		fireEvent.change(keys.getByRole("textbox", { name: "Key name" }), {
			target: { value: "ci-bot" },
		});
		fireEvent.click(keys.getByRole("button", { name: "Make key" }));
		expect(await keys.findByText(/^Made ci-bot\. Its file is \//)).toBeInTheDocument();
		expect(await keys.findByText("ci-bot")).toBeInTheDocument();
		const listed = await source.listKeys(null);
		expect(listed.status === "ok" && listed.data.map((k) => k.name)).toContain("ci-bot");
	});

	it("revokes a key", async () => {
		const source = renderApp();
		const keys = await openAgentTools();
		const slack = (await keys.findByText("slack-bot")).closest("div")!.parentElement!;
		fireEvent.click(within(slack).getByRole("button", { name: "Revoke" }));
		fireEvent.click(within(slack).getByRole("button", { name: "Revoke" }));
		await waitFor(() => expect(keys.queryByText("slack-bot")).toBeNull());
		const listed = await source.listKeys(null);
		expect(listed.status === "ok" && listed.data.map((k) => k.name)).toEqual(["raycast"]);
	});

	it("lists the agents allowed to run agents and revokes one once you confirm", async () => {
		const source = renderApp();
		await openAgentTools();
		const agents = within(screen.getByRole("region", { name: "Agents allowed to run agents" }));
		const row = (await agents.findByText("Release checklist")).closest("div")!.parentElement!;
		expect(row).toHaveTextContent(/On .+\. No limits/);
		fireEvent.click(within(row).getByRole("button", { name: "Revoke" }));
		expect(within(row).getByText("It can't start agents after this.")).toBeInTheDocument();
		expect(await held(source, "agent-release")).toBeDefined();
		fireEvent.click(within(row).getByRole("button", { name: "Revoke" }));
		await waitFor(() => expect(agents.queryByText("Release checklist")).toBeNull());
		expect(await agents.findByText("None yet.")).toBeInTheDocument();
		expect(await held(source, "agent-release")).toBeUndefined();
	});

	it("saves the depth new permissions start with", async () => {
		const source = renderApp();
		await openAgentTools();
		const defaults = within(screen.getByRole("region", { name: "New permissions" }));
		fireEvent.change(await defaults.findByLabelText("Depth"), { target: { value: "2" } });
		fireEvent.click(defaults.getByRole("button", { name: "Save defaults" }));
		expect(await defaults.findByText("Saved")).toBeInTheDocument();
		expect((await source.agentDefaults()).depth).toBe(2);
	});
});
