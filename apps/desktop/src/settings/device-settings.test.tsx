// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import type { DataSource } from "@maxhq/shared/data/source";
import type { Entity, NetworkAccess } from "@maxhq/shared/bindings";
import { MOCK_TEST_HOST_SETTINGS } from "@maxhq/shared/data/mock-test-hosts";
import { NO_LIMIT_WARNING } from "@maxhq/shared/settings/memory";
import { DEVBOX, MAC } from "@maxhq/shared/data/mock-view";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

// The mock source with a second network and pool on devbox.
const twoOfEach = (source: DataSource): DataSource => ({
	...source,
	snapshot: async () => {
		const snapshot = await source.snapshot();
		const entities = snapshot.entities.map((e): Entity =>
			e.entity === "device" && e.id === "dev-devbox"
				? {
						...e,
						workspace_host: {
							networks: ["agentbr0", "incusbr-lab"],
							pools: ["agents", "fast"],
						},
					}
				: e
		);
		return { ...snapshot, entities };
	},
});

const renderDevice = (link: string, scenario = "connected", wrap = (s: DataSource) => s) => {
	const source = wrap(mockSource({ now: () => NOW, tick: false, scenario }));
	const ui = createUiStore();
	ui.getState().setMode("devices");
	ui.getState().selectLink(link);
	render(
		<FixedNow value={NOW}>
			<App source={source} ui={ui} />
		</FixedNow>
	);
	return { source };
};

const section = (name: string) => screen.findByRole("region", { name: `Settings for ${name}` });
const mode = async (name: string) =>
	within(await section(name)).getByRole("combobox", { name: /^Mode,/ });

const workspace = (name: string) =>
	screen.findByRole("region", { name: `Workspace settings for ${name}` });

const choose = async (combobox: HTMLElement, text: string) => {
	fireEvent.click(combobox);
	const found = (await screen.findAllByRole("option")).find((o) => o.textContent?.includes(text));
	if (!found) throw new Error(`No option "${text}"`);
	fireEvent.pointerDown(found, { pointerType: "mouse" });
	fireEvent.click(found);
};

const pick = async (name: string, text: string) => choose(await mode(name), text);

const inbox = (name: string) => screen.findByRole("region", { name: `Inbox settings for ${name}` });

// The mock source with devbox's maxd from before inboxes.
const oldMaxd = (source: DataSource): DataSource => ({
	...source,
	snapshot: async () => {
		const snapshot = await source.snapshot();
		const entities = snapshot.entities.map((e): Entity =>
			e.entity === "device_link" && e.device === "dev-devbox" && e.capabilities
				? { ...e, capabilities: { ...e.capabilities, inbox: false } }
				: e
		);
		return { ...snapshot, entities };
	},
});

describe("device settings", () => {
	it("shows the default each device reports", async () => {
		renderDevice("link-mac");
		expect(await mode("Mac")).toHaveTextContent("Auto");
		expect(
			within(await section("Mac")).getByText(
				"Auto on the Mac and Full access on remote devices by default"
			)
		).toBeInTheDocument();
	});

	it("saves a new default mode on the device", async () => {
		const { source } = renderDevice("link-mac");
		const save = vi.spyOn(source, "setDeviceSettings");
		await pick("Mac", "Ask first");

		expect(save).toHaveBeenCalledWith({
			device: "dev-mac",
			default_permission: "ask",
			workspace_network: null,
			workspace_pool: null,
			auto_settle_days: 5,
			snooze_at_limit: false,
			continue_claude_at_limit: false,
			continue_codex_at_limit: false,
			claude_effort: null,
			codex_effort: null,
			placement: "normal",
			tool_updates: [],
			cursor_usage: false,
			keep_awake: "charger",
			idle_release_minutes: 60,
			memory_items: 256,
			history_mib: 256,
			claude_setup_token: false,
			shell_integration: true,
			read_other_agents: true,
			skill_library: null,
			network: {},
			test_hosts: MOCK_TEST_HOST_SETTINGS[MAC],
		});
		expect(await within(await section("Mac")).findByText("Saved")).toBeInTheDocument();
		expect(await mode("Mac")).toHaveTextContent("Ask first");
		const { entities } = await source.snapshot();
		expect(entities).toContainEqual({
			entity: "device_settings",
			device: "dev-mac",
			default_permission: "ask",
			workspace_network: null,
			workspace_pool: null,
			auto_settle_days: 5,
			snooze_at_limit: false,
			continue_claude_at_limit: false,
			continue_codex_at_limit: false,
			claude_effort: null,
			codex_effort: null,
			placement: "normal",
			tool_updates: [],
			cursor_usage: false,
			keep_awake: "charger",
			idle_release_minutes: 60,
			memory_items: 256,
			history_mib: 256,
			claude_setup_token: false,
			shell_integration: true,
			read_other_agents: true,
			skill_library: null,
			network: {},
			test_hosts: MOCK_TEST_HOST_SETTINGS[MAC],
		});
	});

	it("asks before Auto becomes the default the first time", async () => {
		const { source } = renderDevice("link-devbox");
		const save = vi.spyOn(source, "setDeviceSettings");
		await pick("devbox", "Auto");
		const dialog = await screen.findByRole("alertdialog");
		expect(dialog).toHaveTextContent("Use Auto mode?");
		fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
		await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
		expect(save).not.toHaveBeenCalled();

		await pick("devbox", "Auto");
		fireEvent.click(
			within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Use Auto" })
		);
		await waitFor(() =>
			expect(save).toHaveBeenCalledWith(
				expect.objectContaining({ device: "dev-devbox", default_permission: "auto" })
			)
		);
	});

	it("keeps a device's workspace defaults when the mode changes", async () => {
		const { source } = renderDevice("link-devbox");
		const save = vi.spyOn(source, "setDeviceSettings");
		await pick("devbox", "Ask first");

		expect(save).toHaveBeenCalledWith({
			device: "dev-devbox",
			default_permission: "ask",
			workspace_network: "agentbr0",
			workspace_pool: "agents",
			auto_settle_days: 5,
			snooze_at_limit: false,
			continue_claude_at_limit: false,
			continue_codex_at_limit: false,
			claude_effort: null,
			codex_effort: null,
			placement: "normal",
			tool_updates: [],
			cursor_usage: false,
			keep_awake: "charger",
			idle_release_minutes: 60,
			memory_items: 256,
			history_mib: 256,
			claude_setup_token: true,
			shell_integration: true,
			read_other_agents: true,
			skill_library: null,
			network: {},
			test_hosts: MOCK_TEST_HOST_SETTINGS[DEVBOX],
		});
	});

	it("says why it didn't save and shows the mode the device still has", async () => {
		const { source } = renderDevice("link-mac");
		vi.spyOn(source, "setDeviceSettings").mockResolvedValue({
			status: "error",
			error: { kind: "failed", message: "maxd couldn't write its settings" },
		});
		await pick("Mac", "Plan only");

		expect(
			await within(await section("Mac")).findByText("maxd couldn't write its settings")
		).toBeInTheDocument();
		expect(await mode("Mac")).toHaveTextContent("Auto");
	});

	it("can't change a device that isn't connected", async () => {
		renderDevice("link-devbox", "tailscale_off");
		expect(await mode("devbox")).toBeDisabled();
		expect(
			within(await section("devbox")).getByText("Connect to devbox to change it.")
		).toBeInTheDocument();
	});

	it("shows the network and pool new workspaces get", async () => {
		renderDevice("link-devbox");
		const section = await workspace("devbox");
		expect(
			within(section).getByRole("combobox", { name: "Network, agentbr0" })
		).toBeInTheDocument();
		expect(
			within(section).getByRole("combobox", { name: "Storage pool, agents" })
		).toBeInTheDocument();
	});

	it("saves a new network and keeps the mode and pool", async () => {
		const { source } = renderDevice("link-devbox", "connected", twoOfEach);
		const save = vi.spyOn(source, "setDeviceSettings");
		const section = await workspace("devbox");
		await choose(within(section).getByRole("combobox", { name: /^Network,/ }), "incusbr-lab");

		expect(save).toHaveBeenCalledWith({
			device: "dev-devbox",
			default_permission: "full_access",
			workspace_network: "incusbr-lab",
			workspace_pool: "agents",
			auto_settle_days: 5,
			snooze_at_limit: false,
			continue_claude_at_limit: false,
			continue_codex_at_limit: false,
			claude_effort: null,
			codex_effort: null,
			placement: "normal",
			tool_updates: [],
			cursor_usage: false,
			keep_awake: "charger",
			idle_release_minutes: 60,
			memory_items: 256,
			history_mib: 256,
			claude_setup_token: true,
			shell_integration: true,
			read_other_agents: true,
			skill_library: null,
			network: {},
			test_hosts: MOCK_TEST_HOST_SETTINGS[DEVBOX],
		});
		expect(await within(section).findByText("Saved")).toBeInTheDocument();
	});

	it("saves a new pool and keeps the network", async () => {
		const { source } = renderDevice("link-devbox", "connected", twoOfEach);
		const save = vi.spyOn(source, "setDeviceSettings");
		const section = await workspace("devbox");
		await choose(within(section).getByRole("combobox", { name: /^Storage pool,/ }), "fast");

		expect(save).toHaveBeenCalledWith({
			device: "dev-devbox",
			default_permission: "full_access",
			workspace_network: "agentbr0",
			workspace_pool: "fast",
			auto_settle_days: 5,
			snooze_at_limit: false,
			continue_claude_at_limit: false,
			continue_codex_at_limit: false,
			claude_effort: null,
			codex_effort: null,
			placement: "normal",
			tool_updates: [],
			cursor_usage: false,
			keep_awake: "charger",
			idle_release_minutes: 60,
			memory_items: 256,
			history_mib: 256,
			claude_setup_token: true,
			shell_integration: true,
			read_other_agents: true,
			skill_library: null,
			network: {},
			test_hosts: MOCK_TEST_HOST_SETTINGS[DEVBOX],
		});
	});

	it("shows when idle agents settle and whether they snooze at the limit", async () => {
		renderDevice("link-mac");
		const section = await inbox("Mac");
		expect(within(section).getByRole("textbox", { name: "Auto-settle after" })).toHaveValue(
			"5"
		);
		expect(
			within(section).getByRole("switch", { name: "Snooze at the limit" })
		).not.toBeChecked();
	});

	it("saves new days once you leave the field, keeping the rest", async () => {
		const { source } = renderDevice("link-devbox");
		const save = vi.spyOn(source, "setDeviceSettings");
		const section = await inbox("devbox");
		const days = within(section).getByRole("textbox", { name: "Auto-settle after" });
		fireEvent.change(days, { target: { value: "1" } });
		fireEvent.change(days, { target: { value: "14" } });
		expect(save).not.toHaveBeenCalled();
		fireEvent.blur(days);

		expect(save).toHaveBeenCalledExactlyOnceWith({
			device: "dev-devbox",
			default_permission: "full_access",
			workspace_network: "agentbr0",
			workspace_pool: "agents",
			auto_settle_days: 14,
			snooze_at_limit: false,
			continue_claude_at_limit: false,
			continue_codex_at_limit: false,
			claude_effort: null,
			codex_effort: null,
			placement: "normal",
			tool_updates: [],
			cursor_usage: false,
			keep_awake: "charger",
			idle_release_minutes: 60,
			memory_items: 256,
			history_mib: 256,
			claude_setup_token: true,
			shell_integration: true,
			read_other_agents: true,
			skill_library: null,
			network: {},
			test_hosts: MOCK_TEST_HOST_SETTINGS[DEVBOX],
		});
		expect(await within(section).findByText("Saved")).toBeInTheDocument();
		expect(days).toHaveValue("14");
	});

	it("takes 0 for never, on Enter", async () => {
		const { source } = renderDevice("link-mac");
		const save = vi.spyOn(source, "setDeviceSettings");
		const days = within(await inbox("Mac")).getByRole("textbox", { name: "Auto-settle after" });
		fireEvent.change(days, { target: { value: "0" } });
		fireEvent.keyDown(days, { key: "Enter" });
		expect(save).toHaveBeenCalledWith(expect.objectContaining({ auto_settle_days: 0 }));
	});

	it("says why it won't take days past 90 and keeps the days the device has", async () => {
		const { source } = renderDevice("link-mac");
		const save = vi.spyOn(source, "setDeviceSettings");
		const section = await inbox("Mac");
		const days = within(section).getByRole("textbox", { name: "Auto-settle after" });
		fireEvent.change(days, { target: { value: "120" } });
		fireEvent.blur(days);

		expect(save).not.toHaveBeenCalled();
		expect(
			within(section).getByText("120 isn't a number of days. Use 0 to 90.")
		).toBeInTheDocument();
		expect(days).toHaveValue("5");
	});

	it("saves snoozing at the limit", async () => {
		const { source } = renderDevice("link-mac");
		const save = vi.spyOn(source, "setDeviceSettings");
		const section = await inbox("Mac");
		fireEvent.click(within(section).getByRole("switch", { name: "Snooze at the limit" }));

		expect(save).toHaveBeenCalledWith({
			device: "dev-mac",
			default_permission: "auto",
			workspace_network: null,
			workspace_pool: null,
			auto_settle_days: 5,
			snooze_at_limit: true,
			continue_claude_at_limit: false,
			continue_codex_at_limit: false,
			claude_effort: null,
			codex_effort: null,
			placement: "normal",
			tool_updates: [],
			cursor_usage: false,
			keep_awake: "charger",
			idle_release_minutes: 60,
			memory_items: 256,
			history_mib: 256,
			claude_setup_token: false,
			shell_integration: true,
			read_other_agents: true,
			skill_library: null,
			network: {},
			test_hosts: MOCK_TEST_HOST_SETTINGS[MAC],
		});
		expect(await within(section).findByText("Saved")).toBeInTheDocument();
		expect(within(section).getByRole("switch", { name: "Snooze at the limit" })).toBeChecked();
	});

	it("asks for a newer maxd before it changes the inbox settings", async () => {
		renderDevice("link-devbox", "connected", oldMaxd);
		const section = await inbox("devbox");
		expect(within(section).getByRole("textbox", { name: "Auto-settle after" })).toBeDisabled();
		expect(
			within(section).getByRole("switch", { name: "Snooze at the limit" })
		).toHaveAttribute("aria-disabled", "true");
		expect(
			within(section).getAllByText("devbox's maxd is too old for these. Update it first.")
		).toHaveLength(2);
		expect(await mode("devbox")).toBeEnabled();
	});

	it("can't change the inbox settings of a device that isn't connected", async () => {
		renderDevice("link-devbox", "tailscale_off");
		const section = await inbox("devbox");
		expect(within(section).getByRole("textbox", { name: "Auto-settle after" })).toBeDisabled();
		expect(within(section).getAllByText("Connect to devbox to change it.")).toHaveLength(2);
	});

	it("turns Cursor usage on from the Mac", async () => {
		const { source } = renderDevice("link-mac");
		const save = vi.spyOn(source, "setDeviceSettings");
		const region = await screen.findByRole("region", { name: "Cursor usage for Mac" });
		expect(
			within(region).getByText(/reads the Cursor CLI's saved login from the Keychain/)
		).toBeInTheDocument();
		const toggle = within(region).getByRole("switch", { name: "Enable Cursor usage" });
		expect(toggle).not.toBeChecked();
		fireEvent.click(toggle);

		expect(save).toHaveBeenCalledWith(
			expect.objectContaining({
				device: "dev-mac",
				cursor_usage: true,
				skill_library: null,
				snooze_at_limit: false,
				continue_claude_at_limit: false,
				continue_codex_at_limit: false,
				claude_effort: null,
				codex_effort: null,
			})
		);
		expect(await within(region).findByText("Saved")).toBeInTheDocument();
		expect(within(region).getByRole("switch", { name: "Enable Cursor usage" })).toBeChecked();
	});

	it("offers Cursor usage only on a Mac", async () => {
		renderDevice("link-devbox");
		await section("devbox");
		expect(screen.queryByRole("region", { name: "Cursor usage for devbox" })).toBeNull();
	});

	it("has no workspace settings for a device without Incus", async () => {
		renderDevice("link-mac");
		await section("Mac");
		expect(screen.queryByRole("region", { name: "Workspace settings for Mac" })).toBeNull();
	});
});

describe("keep awake", () => {
	const sleep = (name: string) =>
		screen.findByRole("region", { name: `Sleep settings for ${name}` });
	const keepAwake = async (name: string) =>
		within(await sleep(name)).getByRole("combobox", { name: /^Keep awake,/ });

	// The mock source with these changes to each entity.
	const changed =
		(change: (entity: Entity) => Entity) =>
		(source: DataSource): DataSource => ({
			...source,
			snapshot: async () => {
				const snapshot = await source.snapshot();
				return { ...snapshot, entities: snapshot.entities.map(change) };
			},
		});

	it("reads On the charger by default, with why it matters", async () => {
		renderDevice("link-mac");
		expect(await keepAwake("Mac")).toHaveTextContent("On the charger");
		const region = await sleep("Mac");
		expect(within(region).getByText("Keep this Mac awake")).toBeInTheDocument();
		expect(
			within(region).getByText(
				"A sleeping Mac stops maxd, so your phone, webhooks and scheduled tasks can't reach it. The display still turns off. Closing the lid still sleeps the Mac."
			)
		).toBeInTheDocument();
	});

	it("reads On the charger for a maxd from before the setting", async () => {
		const old = changed((e) => {
			if (e.entity !== "device_settings") return e;
			const rest = { ...e };
			delete rest.keep_awake;
			return rest;
		});
		renderDevice("link-mac", "connected", old);
		expect(await keepAwake("Mac")).toHaveTextContent("On the charger");
	});

	it("saves a change, keeping the other settings", async () => {
		const { source } = renderDevice("link-mac");
		const save = vi.spyOn(source, "setDeviceSettings");
		await choose(await keepAwake("Mac"), "Always");

		expect(save).toHaveBeenCalledWith(
			expect.objectContaining({
				device: "dev-mac",
				keep_awake: "always",
				default_permission: "auto",
				cursor_usage: false,
			})
		);
		const region = await sleep("Mac");
		expect(await within(region).findByText("Saved")).toBeInTheDocument();
		expect(await keepAwake("Mac")).toHaveTextContent("Always");
	});

	it("shows each option's whole description", async () => {
		renderDevice("link-mac");
		fireEvent.click(await keepAwake("Mac"));
		for (const text of [
			"Awake whenever it's plugged in. On battery, only while an agent is working.",
			"Awake whenever maxd runs, on battery too. A Mac left unplugged runs until its battery is low.",
			"Awake only while an agent on this Mac is working.",
			"MaxHQ never keeps this Mac awake, even while an agent works.",
		]) {
			expect(await screen.findByText(text)).not.toHaveClass("truncate");
		}
	});

	it("shows only on a Mac", async () => {
		renderDevice("link-devbox");
		await section("devbox");
		expect(screen.queryByRole("region", { name: "Sleep settings for devbox" })).toBeNull();
	});

	it("can't change on a Mac that isn't connected", async () => {
		const mac = changed((e) =>
			e.entity === "device" && e.id === "dev-devbox" ? { ...e, os: "macos" } : e
		);
		renderDevice("link-devbox", "tailscale_off", mac);
		expect(await keepAwake("devbox")).toBeDisabled();
		const region = await sleep("devbox");
		expect(within(region).getByText("Connect to devbox to change it.")).toBeInTheDocument();
	});
});

describe("idle agents", () => {
	const idle = (name: string) => screen.findByRole("region", { name: `Idle agents on ${name}` });
	const after = async (name: string) =>
		within(await idle(name)).getByRole("combobox", { name: /^Stop idle CLIs after,/ });

	// The mock source with devbox's maxd from before the setting.
	const before = (source: DataSource): DataSource => ({
		...source,
		snapshot: async () => {
			const snapshot = await source.snapshot();
			const entities = snapshot.entities.map((e): Entity => {
				if (e.entity !== "device_settings" || e.device !== "dev-devbox") return e;
				const rest = { ...e };
				delete rest.idle_release_minutes;
				return rest;
			});
			return { ...snapshot, entities };
		},
	});

	it("reads 1 hour by default, with what it frees", async () => {
		renderDevice("link-devbox");
		expect(await after("devbox")).toHaveTextContent("1 hour");
		expect(
			within(await idle("devbox")).getByText(
				"maxd stops the CLI of an agent idle this long to free its memory. The agent stays as it is and starts again with your next message."
			)
		).toBeInTheDocument();
	});

	it("saves Never, keeping the other settings", async () => {
		const { source } = renderDevice("link-mac");
		const save = vi.spyOn(source, "setDeviceSettings");
		await choose(await after("Mac"), "Never");

		expect(save).toHaveBeenCalledWith(
			expect.objectContaining({
				device: "dev-mac",
				idle_release_minutes: 0,
				keep_awake: "charger",
				default_permission: "auto",
			})
		);
		expect(await within(await idle("Mac")).findByText("Saved")).toBeInTheDocument();
		expect(await after("Mac")).toHaveTextContent("Never");
	});

	it("can't change on a maxd from before the setting", async () => {
		renderDevice("link-devbox", "connected", before);
		expect(await after("devbox")).toBeDisabled();
		expect(
			within(await idle("devbox")).getByText(
				"devbox's maxd is too old for this. Update it first."
			)
		).toBeInTheDocument();
	});
});

describe("usage limits", () => {
	const limits = (name: string) =>
		screen.findByRole("region", { name: `Usage limits on ${name}` });
	const claude = async (name: string) =>
		within(await limits(name)).getByRole("switch", {
			name: "Claude Code agents continue after the limit",
		});
	const codex = async (name: string) =>
		within(await limits(name)).getByRole("switch", {
			name: "Codex agents continue after the limit",
		});

	// The mock source with devbox's maxd from before the setting.
	const before = (source: DataSource): DataSource => ({
		...source,
		snapshot: async () => {
			const snapshot = await source.snapshot();
			const entities = snapshot.entities.map((e): Entity => {
				if (e.entity !== "device_settings" || e.device !== "dev-devbox") return e;
				const rest = { ...e };
				delete rest.continue_claude_at_limit;
				delete rest.continue_codex_at_limit;
				return rest;
			});
			return { ...snapshot, entities };
		},
	});

	it("is off by default for each vendor, with what it does", async () => {
		renderDevice("link-devbox");
		expect(await claude("devbox")).not.toBeChecked();
		expect(await codex("devbox")).not.toBeChecked();
		expect(
			within(await limits("devbox")).getByText(
				"An agent that a usage limit stops mid-turn gets a message to carry on once the limit resets. This is the default; turn it on or off for one agent from its menu in the sidebar."
			)
		).toBeInTheDocument();
	});

	it("saves one vendor on, keeping the other settings", async () => {
		const { source } = renderDevice("link-mac");
		const save = vi.spyOn(source, "setDeviceSettings");
		fireEvent.click(await claude("Mac"));

		expect(save).toHaveBeenCalledWith(
			expect.objectContaining({
				device: "dev-mac",
				continue_claude_at_limit: true,
				continue_codex_at_limit: false,
				snooze_at_limit: false,
				default_permission: "auto",
			})
		);
		expect(await within(await limits("Mac")).findByText("Saved")).toBeInTheDocument();
		expect(await claude("Mac")).toBeChecked();
		expect(await codex("Mac")).not.toBeChecked();
	});

	it("saves Codex on its own", async () => {
		const { source } = renderDevice("link-mac");
		const save = vi.spyOn(source, "setDeviceSettings");
		fireEvent.click(await codex("Mac"));

		expect(save).toHaveBeenCalledWith(
			expect.objectContaining({
				continue_claude_at_limit: false,
				continue_codex_at_limit: true,
			})
		);
	});

	it("can't change on a maxd from before the setting", async () => {
		renderDevice("link-devbox", "connected", before);
		expect(await claude("devbox")).toHaveAttribute("aria-disabled", "true");
		expect(await codex("devbox")).toHaveAttribute("aria-disabled", "true");
		expect(
			within(await limits("devbox")).getAllByText(
				"devbox's maxd is too old for this. Update it first."
			)
		).toHaveLength(2);
	});

	it("can't change on a device that isn't connected", async () => {
		renderDevice("link-devbox", "tailscale_off");
		expect(await claude("devbox")).toHaveAttribute("aria-disabled", "true");
		expect(
			within(await limits("devbox")).getAllByText("Connect to devbox to change it.")
		).toHaveLength(2);
	});
});

describe("memory", () => {
	const memory = (name: string) => screen.findByRole("region", { name: `Memory on ${name}` });
	const items = async (name: string) =>
		within(await memory(name)).getByRole("combobox", {
			name: /^Recent items kept in memory per agent,/,
		});
	const history = async (name: string) =>
		within(await memory(name)).getByRole("combobox", { name: /^History kept per agent,/ });
	const warning = NO_LIMIT_WARNING;

	it("reads 256 items and 256 MB by default, with what each means", async () => {
		renderDevice("link-devbox");
		expect(await items("devbox")).toHaveTextContent("256 items");
		expect(await history("devbox")).toHaveTextContent("256 MB");
		const region = await memory("devbox");
		expect(
			within(region).getByText(
				"maxd keeps this many of each agent's newest items in memory, and the rest on disk until a client scrolls back. More uses more memory per agent."
			)
		).toBeInTheDocument();
		expect(within(region).queryByText(warning)).toBeNull();
	});

	it("saves more items, keeping the other settings", async () => {
		const { source } = renderDevice("link-mac");
		const save = vi.spyOn(source, "setDeviceSettings");
		await choose(await items("Mac"), "4,096 items");

		expect(save).toHaveBeenCalledWith(
			expect.objectContaining({
				device: "dev-mac",
				memory_items: 4096,
				history_mib: 256,
				claude_setup_token: false,
				idle_release_minutes: 60,
			})
		);
		expect(await within(await memory("Mac")).findByText("Saved")).toBeInTheDocument();
		expect(await items("Mac")).toHaveTextContent("4,096 items");
	});

	it("saves No limit and says what it risks", async () => {
		const { source } = renderDevice("link-mac");
		const save = vi.spyOn(source, "setDeviceSettings");
		await choose(await history("Mac"), "No limit");

		expect(save).toHaveBeenCalledWith(
			expect.objectContaining({ device: "dev-mac", history_mib: 0, memory_items: 256 })
		);
		expect(await history("Mac")).toHaveTextContent("No limit");
		expect(within(await memory("Mac")).getByText(warning)).toBeInTheDocument();
	});

	it("can't change on a device that isn't connected", async () => {
		renderDevice("link-devbox", "tailscale_off");
		expect(await items("devbox")).toBeDisabled();
		expect(await history("devbox")).toBeDisabled();
		const region = await memory("devbox");
		expect(within(region).getAllByText("Connect to devbox to change it.")).toHaveLength(2);
	});
});

describe("terminals", () => {
	const terminals = (name: string) =>
		screen.findByRole("region", { name: `Terminals on ${name}` });
	const shell = async (name: string) =>
		within(await terminals(name)).getByRole("switch", { name: "Shell integration" });

	// The mock source with devbox's maxd from before the setting.
	const before = (source: DataSource): DataSource => ({
		...source,
		snapshot: async () => {
			const snapshot = await source.snapshot();
			const entities = snapshot.entities.map((e): Entity => {
				if (e.entity !== "device_settings" || e.device !== "dev-devbox") return e;
				const rest = { ...e };
				delete rest.shell_integration;
				return rest;
			});
			return { ...snapshot, entities };
		},
	});

	it("is on by default, with what it does", async () => {
		renderDevice("link-devbox");
		expect(await shell("devbox")).toBeChecked();
		expect(
			within(await terminals("devbox")).getByText(/^MaxHQ adds hooks to zsh, bash and fish/)
		).toBeInTheDocument();
	});

	it("saves it off, keeping the other settings", async () => {
		const { source } = renderDevice("link-mac");
		const save = vi.spyOn(source, "setDeviceSettings");
		fireEvent.click(await shell("Mac"));

		expect(save).toHaveBeenCalledWith(
			expect.objectContaining({
				device: "dev-mac",
				shell_integration: false,
				memory_items: 256,
				default_permission: "auto",
			})
		);
		expect(await within(await terminals("Mac")).findByText("Saved")).toBeInTheDocument();
		expect(await shell("Mac")).not.toBeChecked();
	});

	it("can't change on a maxd from before the setting, which adds no hooks", async () => {
		renderDevice("link-devbox", "connected", before);
		const toggle = await shell("devbox");
		expect(toggle).toHaveAttribute("aria-disabled", "true");
		expect(toggle).not.toBeChecked();
		expect(
			within(await terminals("devbox")).getByText(
				"devbox's maxd is too old for this. Update it first."
			)
		).toBeInTheDocument();
	});
});

describe("other agents", () => {
	const card = (name: string) => screen.findByRole("region", { name: `Other agents on ${name}` });
	const reads = async (name: string) =>
		within(await card(name)).getByRole("switch", { name: "Agents can read other agents" });

	// The mock source with devbox's maxd from before the setting.
	const before = (source: DataSource): DataSource => ({
		...source,
		snapshot: async () => {
			const snapshot = await source.snapshot();
			const entities = snapshot.entities.map((e): Entity => {
				if (e.entity !== "device_settings" || e.device !== "dev-devbox") return e;
				const rest = { ...e };
				delete rest.read_other_agents;
				return rest;
			});
			return { ...snapshot, entities };
		},
	});

	it("is on by default, and says what agents can see", async () => {
		renderDevice("link-devbox");
		expect(await reads("devbox")).toBeChecked();
		expect(
			within(await card("devbox")).getByText(
				/^Any agent can list and read any other agent's conversation, which may hold code, paths and command output\./
			)
		).toBeInTheDocument();
	});

	it("saves it off, keeping the other settings", async () => {
		const { source } = renderDevice("link-mac");
		const save = vi.spyOn(source, "setDeviceSettings");
		fireEvent.click(await reads("Mac"));

		expect(save).toHaveBeenCalledWith(
			expect.objectContaining({
				device: "dev-mac",
				read_other_agents: false,
				shell_integration: true,
				default_permission: "auto",
			})
		);
		expect(await within(await card("Mac")).findByText("Saved")).toBeInTheDocument();
		expect(await reads("Mac")).not.toBeChecked();
	});

	it("can't change on a maxd from before the setting", async () => {
		renderDevice("link-devbox", "connected", before);
		const toggle = await reads("devbox");
		expect(toggle).toHaveAttribute("aria-disabled", "true");
		expect(toggle).not.toBeChecked();
		expect(
			within(await card("devbox")).getByText(
				"devbox's maxd is too old for this. Update it first."
			)
		).toBeInTheDocument();
	});
});

describe("placement", () => {
	it("saves Prefer and keeps the rest", async () => {
		const { source } = renderDevice("link-devbox");
		const save = vi.spyOn(source, "setDeviceSettings");
		const region = await screen.findByRole("region", { name: "Placement for devbox" });
		const placement = within(region).getByRole("combobox", { name: "Placement, Normal" });
		fireEvent.click(placement);
		const prefer = (await screen.findAllByRole("option")).find((o) =>
			o.textContent?.startsWith("Prefer")
		);
		if (!prefer) throw new Error("No Prefer option");
		fireEvent.pointerDown(prefer, { pointerType: "mouse" });
		fireEvent.click(prefer);

		expect(save).toHaveBeenCalledWith(
			expect.objectContaining({
				device: "dev-devbox",
				placement: "prefer",
				workspace_network: "agentbr0",
			})
		);
		expect(await within(region).findByText("Saved")).toBeInTheDocument();
		expect(
			within(region).getByRole("combobox", { name: "Placement, Prefer" })
		).toBeInTheDocument();
	});
});

describe("skill library", () => {
	const library = (name: string) =>
		screen.findByRole("region", { name: `Skill library for ${name}` });

	it("shows which device holds it, and the holder can't turn it off", async () => {
		renderDevice("link-mac");
		const region = await library("Mac");
		const toggle = within(region).getByRole("switch", { name: "Holds the skill library" });
		expect(toggle).toBeChecked();
		expect(toggle).toHaveAttribute("aria-disabled", "true");
		expect(
			within(region).getByText(
				"Mac holds the skill library. To move it, turn this on for another device."
			)
		).toBeInTheDocument();
	});

	it("moves it to another device", async () => {
		const { source } = renderDevice("link-devbox");
		const move = vi.spyOn(source, "moveSkillLibrary");
		const save = vi.spyOn(source, "setDeviceSettings");
		const region = await library("devbox");
		expect(
			within(region).getByText(
				"Mac holds it now. Turning this on moves it and its sources to devbox."
			)
		).toBeInTheDocument();
		const toggle = within(region).getByRole("switch", { name: "Holds the skill library" });
		expect(toggle).not.toBeChecked();
		fireEvent.click(toggle);

		expect(move).toHaveBeenCalledWith("dev-devbox");
		expect(save).not.toHaveBeenCalled();
		expect(await within(region).findByText("Moved")).toBeInTheDocument();
		expect(
			within(region).getByRole("switch", { name: "Holds the skill library" })
		).toBeChecked();
	});

	it("says why it didn't move", async () => {
		const { source } = renderDevice("link-devbox");
		const reason =
			"Mac has approved skill changes it hasn't pushed. Push the library's approvals first.";
		vi.spyOn(source, "moveSkillLibrary").mockResolvedValue({
			status: "ok",
			data: { status: "refused", reason },
		});
		const region = await library("devbox");
		fireEvent.click(within(region).getByRole("switch", { name: "Holds the skill library" }));
		expect(await within(region).findByText(reason)).toBeInTheDocument();
		expect(
			within(region).getByRole("switch", { name: "Holds the skill library" })
		).not.toBeChecked();
	});
});

describe("device links", () => {
	it("turns a link off and allows it again", async () => {
		renderDevice("link-devbox");
		const region = await screen.findByRole("region", { name: "Device links for devbox" });
		expect(await within(region).findByText("Linked")).toBeInTheDocument();

		fireEvent.click(within(region).getByRole("button", { name: "Remove MacBook Pro" }));
		expect(await within(region).findByText("Off")).toBeInTheDocument();

		fireEvent.click(await within(region).findByRole("button", { name: "Allow MacBook Pro" }));
		expect(await within(region).findByText("Linked")).toBeInTheDocument();
		expect(within(region).getByRole("button", { name: "Remove MacBook Pro" })).toBeEnabled();
	});
});

describe("network access", () => {
	const region = (name: string) =>
		screen.findByRole("region", { name: `Network access for ${name}` });

	// The mock source with devbox's network access changed by `access`, and
	// `more` besides.
	const devboxAccess =
		(access: Partial<NetworkAccess> | null, more: Entity[] = []) =>
		(source: DataSource): DataSource => ({
			...source,
			snapshot: async () => {
				const snapshot = await source.snapshot();
				const entities = snapshot.entities.flatMap((e): Entity[] =>
					e.entity === "network_access" && e.device === DEVBOX
						? access
							? [{ ...e, ...access }]
							: []
						: [e]
				);
				return { ...snapshot, entities: [...entities, ...more] };
			},
		});

	it("is off until you turn it on, then lists the addresses phones use", async () => {
		const { source } = renderDevice("link-devbox");
		const save = vi.spyOn(source, "setDeviceSettings");
		const section = await region("devbox");
		expect(
			within(section).getByText(
				"Phones on the same network or tailnet can reach devbox while it's on."
			)
		).toBeInTheDocument();
		expect(within(section).getByRole("textbox", { name: "Port" })).toHaveValue("7466");
		expect(within(section).queryByText("Addresses for phones")).toBeNull();

		const toggle = within(section).getByRole("switch", { name: "Network access" });
		expect(toggle).not.toBeChecked();
		fireEvent.click(toggle);

		expect(save).toHaveBeenCalledWith(
			expect.objectContaining({ device: DEVBOX, network: { on: true } })
		);
		expect(await within(section).findByText("Addresses for phones")).toBeInTheDocument();
		expect(within(section).getByText("192.168.1.30")).toBeInTheDocument();
		expect(within(section).getByRole("switch", { name: "Network access" })).toBeChecked();
	});

	it("saves a new port and says why it won't take one that isn't", async () => {
		const { source } = renderDevice("link-devbox");
		const save = vi.spyOn(source, "setDeviceSettings");
		const section = await region("devbox");
		const port = within(section).getByRole("textbox", { name: "Port" });
		fireEvent.change(port, { target: { value: "70000" } });
		fireEvent.blur(port);
		expect(save).not.toHaveBeenCalled();
		expect(
			within(section).getByText("70000 isn't a port. Ports run from 1 to 65535.")
		).toBeInTheDocument();

		fireEvent.change(port, { target: { value: "7500" } });
		fireEvent.keyDown(port, { key: "Enter" });
		expect(save).toHaveBeenCalledExactlyOnceWith(
			expect.objectContaining({ device: DEVBOX, network: { port: 7500 } })
		);
	});

	it("shows what keeps it from listening", async () => {
		const problem = "Port 7466 is in use on this device. Pick another port.";
		renderDevice("link-devbox", "connected", devboxAccess({ on: true, problem }));
		const section = await region("devbox");
		expect(within(section).getByText(problem)).toBeInTheDocument();
		expect(within(section).queryByText("Addresses for phones")).toBeNull();
	});

	it("lists the phones signed in, and Revoke ends a sign-in", async () => {
		const phone: Entity = {
			entity: "phone",
			id: "phone-1",
			name: "Work phone",
			ssh_public: "ssh-ed25519 AAAA",
			push_public: "AAAA",
			paired_at: NOW,
			device: DEVBOX,
			push: [],
		};
		const session: Entity = {
			entity: "phone_session",
			id: "session-1",
			device: DEVBOX,
			phone: "phone-1",
			created: NOW - 60 * 60 * 1000,
			expires: NOW + 29 * 24 * 60 * 60 * 1000,
			last_used: null,
		};
		const { source } = renderDevice(
			"link-devbox",
			"connected",
			devboxAccess({ on: true, hosts: ["192.168.1.30"] }, [phone, session])
		);
		const revoke = vi
			.spyOn(source, "revokePhoneSession")
			.mockResolvedValue({ status: "ok", data: { status: "done" } });
		const section = await region("devbox");
		expect(within(section).getByText("Work phone")).toBeInTheDocument();
		expect(within(section).getByText(/, never used, expires in 29 d$/)).toBeInTheDocument();

		fireEvent.click(within(section).getByRole("button", { name: "Revoke" }));
		fireEvent.click(within(section).getByRole("button", { name: "Revoke" }));
		expect(revoke).toHaveBeenCalledWith("session-1");
	});

	it("has no section on a device from before network access", async () => {
		renderDevice("link-devbox", "connected", devboxAccess(null));
		await section("devbox");
		expect(
			screen.queryByRole("region", { name: "Network access for devbox" })
		).not.toBeInTheDocument();
	});
});

describe("app keys", () => {
	const keys = (name: string) => screen.findByRole("region", { name: `App keys for ${name}` });

	it("warns about a key this Mac didn't add, and removes it", async () => {
		const { source } = renderDevice("link-devbox");
		const remove = vi.spyOn(source, "removeAppKey");
		const region = await keys("devbox");
		expect(
			await within(region).findByText(
				/^devbox has trusted an app key this Mac didn't add since /
			)
		).toBeInTheDocument();
		expect(within(region).getByText(/^This Mac, Sent at setup, added /)).toBeInTheDocument();
		// Only the other Mac's key has a Remove.
		expect(within(region).getAllByRole("button", { name: "Remove" })).toHaveLength(1);
		expect(within(region).queryByRole("button", { name: "Make a new app key" })).toBeNull();

		fireEvent.click(within(region).getByRole("button", { name: "Remove" }));
		fireEvent.click(within(region).getByRole("button", { name: "Remove" }));
		expect(remove).toHaveBeenCalledWith(DEVBOX, expect.stringMatching(/^b3RoZXI/));
		await expect.poll(() => within(region).queryByText("MaxHQ on travel-mac")).toBeNull();
		expect(within(region).getByText("MaxHQ on studio-mac")).toBeInTheDocument();
		expect(within(region).queryByText(/didn't add since/)).toBeNull();
	});

	it("makes a new key from this Mac's page", async () => {
		const { source } = renderDevice("link-mac");
		const rotate = vi.spyOn(source, "rotateAppKey");
		const region = await keys("Mac");
		expect(
			await within(region).findByText(/^Makes a new key for this Mac and moves every device/)
		).toBeInTheDocument();
		expect(within(region).queryByRole("button", { name: "Remove" })).toBeNull();
		fireEvent.click(within(region).getByRole("button", { name: "Make a new app key" }));
		fireEvent.click(within(region).getByRole("button", { name: "Make it" }));
		expect(rotate).toHaveBeenCalled();
		expect(
			await within(region).findByText(/^This Mac, Added by a trusted app, /)
		).toBeInTheDocument();
	});
});

describe("Claude setup-tokens", () => {
	const tokens = (name: string) =>
		screen.findByRole("region", { name: `Claude setup-tokens for ${name}` });
	const toggle = async (name: string) =>
		within(await tokens(name)).getByRole("switch", { name: "Take setup-tokens" });

	// The mock source with devbox's maxd from before the setting.
	const before = (source: DataSource): DataSource => ({
		...source,
		snapshot: async () => {
			const snapshot = await source.snapshot();
			const entities = snapshot.entities.map((e): Entity => {
				if (e.entity !== "device_settings" || e.device !== "dev-devbox") return e;
				const rest = { ...e };
				delete rest.claude_setup_token;
				return rest;
			});
			return { ...snapshot, entities };
		},
	});

	it("is off by default, and says what turning it on means before it does", async () => {
		const { source } = renderDevice("link-mac");
		const save = vi.spyOn(source, "setDeviceSettings");
		expect(
			within(await tokens("Mac")).getByText("Off by default. For personal use")
		).toBeInTheDocument();
		expect(await toggle("Mac")).not.toBeChecked();

		fireEvent.click(await toggle("Mac"));
		let dialog = await screen.findByRole("alertdialog");
		expect(dialog).toHaveTextContent("Turn on setup-tokens for Mac?");
		expect(dialog).toHaveTextContent("This is for personal use.");
		expect(dialog).toHaveTextContent("stores your Claude setup-token on Mac");
		expect(dialog).toHaveTextContent("sign in with claude on each device");
		fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
		await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
		expect(save).not.toHaveBeenCalled();
		expect(await toggle("Mac")).not.toBeChecked();

		fireEvent.click(await toggle("Mac"));
		dialog = await screen.findByRole("alertdialog");
		fireEvent.click(within(dialog).getByRole("button", { name: "Turn on" }));
		await waitFor(() =>
			expect(save).toHaveBeenCalledWith(
				expect.objectContaining({ device: "dev-mac", claude_setup_token: true })
			)
		);
		expect(await within(await tokens("Mac")).findByText("Saved")).toBeInTheDocument();
		expect(await toggle("Mac")).toBeChecked();
	});

	it("turns off without asking", async () => {
		const { source } = renderDevice("link-devbox");
		const save = vi.spyOn(source, "setDeviceSettings");
		expect(await toggle("devbox")).toBeChecked();
		fireEvent.click(await toggle("devbox"));
		await waitFor(() =>
			expect(save).toHaveBeenCalledWith(
				expect.objectContaining({ device: "dev-devbox", claude_setup_token: false })
			)
		);
		expect(screen.queryByRole("alertdialog")).toBeNull();
	});

	it("reads on for a maxd from before the setting, which can't change it", async () => {
		renderDevice("link-devbox", "connected", before);
		const on = await toggle("devbox");
		expect(on).toBeChecked();
		expect(on).toHaveAttribute("data-disabled");
		expect(
			within(await tokens("devbox")).getByText(
				"devbox's maxd is too old for this, and takes setup-tokens. Update it first."
			)
		).toBeInTheDocument();
	});
});
