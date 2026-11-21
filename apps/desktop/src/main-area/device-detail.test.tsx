// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import type { DataSource } from "@maxhq/shared/data/source";
import { type Entity, PROTOCOL_VERSION } from "@maxhq/shared/bindings";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { MIN, NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

// The mock source with devbox winding down until `until`, and its maxd
// speaking `protocol`, when they're set.
const devbox = (
	source: DataSource,
	{ until, protocol }: { until?: number; protocol?: number }
): DataSource => ({
	...source,
	snapshot: async () => {
		const snapshot = await source.snapshot();
		const entities = snapshot.entities.map((e): Entity => {
			if (until !== undefined && e.entity === "device" && e.id === "dev-devbox") {
				return { ...e, winding_down_until: until };
			}
			if (protocol !== undefined && e.entity === "device_link" && e.id === "link-devbox") {
				return { ...e, protocol };
			}
			return e;
		});
		return { ...snapshot, entities };
	},
});

const renderDevice = (
	link: string,
	{
		scenario = "connected",
		until,
		protocol,
		stepMs,
	}: { scenario?: string; until?: number; protocol?: number; stepMs?: number } = {}
) => {
	const source = devbox(mockSource({ now: () => NOW, tick: false, scenario, stepMs }), {
		until,
		protocol,
	});
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

describe("the device page's Settings button", () => {
	it("opens Settings on the device's tab", async () => {
		renderDevice("link-devbox");
		fireEvent.click(await screen.findByRole("button", { name: "Settings for devbox" }));
		const dialog = await screen.findByRole("dialog", { name: "Settings" });
		const tabs = within(dialog).getByRole("tablist", { name: "Device" });
		expect(within(tabs).getByRole("tab", { name: "devbox" })).toHaveAttribute(
			"aria-selected",
			"true"
		);
		expect(
			await within(dialog).findByRole("region", { name: "Idle agents on devbox" })
		).toBeInTheDocument();
	});
});

const restart = () => screen.findByRole("region", { name: "Restart devbox" });
const windDownButton = async () =>
	within(await restart()).getByRole("button", { name: "Wind down for restart" });

describe("wind down for restart", () => {
	it("winds the device down for 20 minutes and shows the banner", async () => {
		const { source } = renderDevice("link-devbox");
		const windDown = vi.spyOn(source, "windDown");
		expect(screen.queryByRole("status", { name: "Winding down" })).toBeNull();
		expect(
			within(await restart()).getByRole("combobox", { name: "Window, 20 min" })
		).toBeInTheDocument();
		fireEvent.click(await windDownButton());

		expect(windDown).toHaveBeenCalledWith("dev-devbox", 1200);
		const banner = await screen.findByRole("status", { name: "Winding down" });
		expect(banner).toHaveTextContent("devbox is winding down for a restart");
		expect(banner).toHaveTextContent(
			"Busy agents are finishing the step they're on. The window closes at 15:08, in 20 min."
		);
		await waitFor(async () => expect(await windDownButton()).toBeDisabled());
	});

	it("sends the window you pick", async () => {
		const { source } = renderDevice("link-devbox");
		const windDown = vi.spyOn(source, "windDown");
		fireEvent.click(within(await restart()).getByRole("combobox", { name: /^Window,/ }));
		const five = (await screen.findAllByRole("option")).find((o) => o.textContent === "5 min");
		if (!five) throw new Error("No 5 min option");
		fireEvent.pointerDown(five, { pointerType: "mouse" });
		fireEvent.click(five);
		fireEvent.click(await windDownButton());
		expect(windDown).toHaveBeenCalledWith("dev-devbox", 300);
	});

	it("says when the window has closed", async () => {
		renderDevice("link-devbox", { until: NOW - 3 * MIN });
		const banner = await screen.findByRole("status", { name: "Winding down" });
		expect(banner).toHaveTextContent("devbox has wound down for a restart");
		expect(banner).toHaveTextContent(
			"The window closed at 14:45. If devbox drops for its restart, MaxHQ tries it every 2 seconds for 10 minutes, and its agents resume once it's back."
		);
		expect(await windDownButton()).toBeEnabled();
	});

	it("says why it didn't wind down", async () => {
		const { source } = renderDevice("link-devbox");
		vi.spyOn(source, "windDown").mockResolvedValue({
			status: "error",
			error: { kind: "failed", message: "maxd couldn't write the restart window" },
		});
		fireEvent.click(await windDownButton());
		expect(
			await within(await restart()).findByText("maxd couldn't write the restart window")
		).toBeInTheDocument();
		expect(screen.queryByRole("status", { name: "Winding down" })).toBeNull();
	});

	it("can't wind down a device that isn't connected", async () => {
		renderDevice("link-devbox", { scenario: "tailscale_off" });
		expect(await windDownButton()).toBeDisabled();
		expect(
			within(await restart()).getByText("Connect to devbox to wind it down.")
		).toBeInTheDocument();
	});
});

describe("restart maxd", () => {
	const restartButton = async () =>
		within(await restart()).getByRole("button", { name: "Restart maxd" });

	it("restarts maxd through the device's link and says so until it's back", async () => {
		// The mock's link drops and connects again over two short steps.
		const { source } = renderDevice("link-devbox", { stepMs: 20 });
		const restartMaxd = vi.spyOn(source, "restartMaxd");
		const row = await restart();
		expect(
			within(row).getByText(
				"Stops maxd on devbox and starts it again. Agents keep running in their own processes and pick up once it's back."
			)
		).toBeInTheDocument();
		fireEvent.click(within(row).getByRole("button", { name: "Restart maxd" }));
		expect(restartMaxd).toHaveBeenCalledWith("link-devbox");
		expect(within(row).getByText("Restarting")).toBeInTheDocument();
		await waitFor(() => expect(within(row).queryByText("Restarting")).toBeNull(), {
			timeout: 5000,
		});
	});

	it("says why it didn't restart", async () => {
		const { source } = renderDevice("link-devbox");
		vi.spyOn(source, "restartMaxd").mockResolvedValue({
			status: "error",
			error: {
				kind: "failed",
				message: "Couldn't reach dev@devbox: SSH sign-in was refused",
			},
		});
		fireEvent.click(await restartButton());
		expect(
			await within(await restart()).findByText(
				"Couldn't reach dev@devbox: SSH sign-in was refused"
			)
		).toBeInTheDocument();
		expect(within(await restart()).queryByText("Restarting")).toBeNull();
		expect(await restartButton()).toBeEnabled();
	});

	it("can restart maxd on a device that isn't connected", async () => {
		const { source } = renderDevice("link-devbox", { scenario: "tailscale_off" });
		const restartMaxd = vi.spyOn(source, "restartMaxd");
		expect(await restartButton()).toBeEnabled();
		fireEvent.click(await restartButton());
		expect(restartMaxd).toHaveBeenCalledWith("link-devbox");
	});
});

describe("remove device", () => {
	const remove = () => screen.findByRole("region", { name: "Remove devbox" });

	it("asks first, then removes the device, which leaves the view", async () => {
		const { source } = renderDevice("link-devbox");
		const removeDevice = vi.spyOn(source, "removeDevice");
		const region = await remove();
		expect(
			within(region).getByText(
				"MaxHQ forgets devbox and closes its link. Your other devices each take out devbox's key, cut their link to it and stop what it started on them, as soon as MaxHQ reaches them. Nothing on devbox is deleted."
			)
		).toBeInTheDocument();
		fireEvent.click(within(region).getByRole("button", { name: "Remove device" }));
		expect(
			within(region).getByText("Remove devbox and cut its link on every device?")
		).toBeInTheDocument();
		fireEvent.click(within(region).getByRole("button", { name: "Keep" }));
		expect(removeDevice).not.toHaveBeenCalled();

		fireEvent.click(within(region).getByRole("button", { name: "Remove device" }));
		fireEvent.click(within(region).getByRole("button", { name: "Remove" }));
		expect(removeDevice).toHaveBeenCalledWith("link-devbox");
		await waitFor(() =>
			expect(screen.queryByRole("region", { name: "Remove devbox" })).toBeNull()
		);
	});

	it("says why it didn't remove the device", async () => {
		const { source } = renderDevice("link-devbox");
		vi.spyOn(source, "removeDevice").mockResolvedValue({
			status: "error",
			error: { kind: "failed", message: "Couldn't write the app's database" },
		});
		const region = await remove();
		fireEvent.click(within(region).getByRole("button", { name: "Remove device" }));
		fireEvent.click(within(region).getByRole("button", { name: "Remove" }));
		expect(
			await within(region).findByText("Couldn't write the app's database")
		).toBeInTheDocument();
	});

	it("offers nothing to remove on this Mac", async () => {
		renderDevice("link-mac");
		await screen.findByRole("button", { name: "Settings for Mac" });
		expect(screen.queryByRole("region", { name: "Remove Mac" })).toBeNull();
	});
});

describe("device workspaces", () => {
	it("lists the workspaces MaxHQ made on devbox", async () => {
		renderDevice("link-devbox");
		const region = await screen.findByRole("region", { name: "Workspaces on devbox" });
		expect(
			within(region).getByRole("group", { name: "Workspace mhq-maxhq" })
		).toBeInTheDocument();
	});

	it("shows no workspaces section for a device without Incus", async () => {
		renderDevice("link-mac");
		await screen.findByRole("button", { name: "Settings for Mac" });
		expect(screen.queryByRole("region", { name: "Workspaces on Mac" })).toBeNull();
	});
});

describe("device load", () => {
	const load = () => screen.findByRole("region", { name: "Load on devbox" });

	it("shows the CPU and memory now and the busiest agents", async () => {
		const { source } = renderDevice("link-devbox");
		const readLoad = vi.spyOn(source, "readLoad");
		const readDiagnostics = vi.spyOn(source, "readDiagnostics");
		const region = await load();
		expect(await within(region).findByRole("group", { name: "CPU" })).toHaveTextContent("71%");
		expect(within(region).getByRole("group", { name: "Cores" })).toHaveTextContent("16");
		expect(within(region).getByRole("group", { name: "Memory free" })).toHaveTextContent(
			"18.0 GB"
		);
		expect(
			await within(region).findByRole("img", { name: "CPU over the last hour" })
		).toBeVisible();
		const table = within(region).getByRole("table", { name: "Busiest processes" });
		expect(within(table).getByText("maxd")).toBeInTheDocument();
		expect(readLoad).toHaveBeenCalledWith(["dev-devbox"]);
		expect(readDiagnostics).toHaveBeenCalledWith("dev-devbox", 3600, 120);
	});

	it("waits for a connection", async () => {
		renderDevice("link-devbox", { scenario: "tailscale_off" });
		expect(within(await load()).getByText("Connect to devbox to see its load.")).toBeVisible();
	});
});

describe("device CLIs", () => {
	const clis = (name = "devbox") =>
		screen.findByRole("region", { name: `Agent CLIs on ${name}` });

	it("updates a CLI MaxHQ installed", async () => {
		const { source } = renderDevice("link-devbox");
		const updateTools = vi.spyOn(source, "updateTools");
		const region = await clis();
		fireEvent.click(within(region).getByRole("button", { name: "Update Claude in mhq-maxhq" }));
		expect(updateTools).toHaveBeenCalledWith("dev-devbox", [
			{ tool: "claude", place: { type: "workspace", workspace: "ws-mhq-maxhq" } },
		]);
		expect(await within(region).findByText("Downloading")).toBeInTheDocument();
	});

	it("updates the host's CLI only once you tick it", async () => {
		const { source } = renderDevice("link-devbox");
		const save = vi.spyOn(source, "setDeviceSettings");
		const region = await clis();
		const update = within(region).getByRole("button", { name: "Update Claude" });
		expect(update).toBeDisabled();
		fireEvent.click(within(region).getByRole("switch", { name: "Let MaxHQ update Claude" }));
		await waitFor(() => expect(update).toBeEnabled());
		expect(save).toHaveBeenCalledWith(
			expect.objectContaining({ device: "dev-devbox", tool_updates: ["claude"] })
		);
	});

	it("shows the command for a CLI you update yourself", async () => {
		renderDevice("link-mac");
		const region = await clis("Mac");
		expect(within(region).getAllByText("claude update")[0]).toBeInTheDocument();
		expect(within(region).queryByRole("button", { name: "Update Claude" })).toBeNull();
		expect(within(region).getAllByRole("button", { name: "Copy command" })).toHaveLength(2);
	});
});

describe("device routes", () => {
	const routes = () => screen.findByRole("region", { name: "Routes to devbox" });
	const hosts = async () =>
		within(within(await routes()).getByRole("list", { name: "SSH hosts" }))
			.getAllByRole("listitem")
			.map((item) => item.querySelector("code")?.textContent);

	it("marks the route that worked last here", async () => {
		renderDevice("link-devbox");
		const items = within(await routes()).getAllByRole("listitem");
		expect(within(items[0]!).getByText("Worked last on this network")).toBeVisible();
		expect(within(items[1]!).queryByText("Worked last on this network")).toBeNull();
	});

	it("moves, adds and removes hosts", async () => {
		const { source } = renderDevice("link-devbox");
		const setRoutes = vi.spyOn(source, "setRoutes");
		fireEvent.click(
			within(await routes()).getByRole("button", { name: "Move dev@devbox.lan up" })
		);
		expect(setRoutes).toHaveBeenLastCalledWith("link-devbox", ["dev@devbox.lan", "dev@devbox"]);
		await waitFor(async () => expect(await hosts()).toEqual(["dev@devbox.lan", "dev@devbox"]));

		fireEvent.change(within(await routes()).getByRole("textbox", { name: "SSH host" }), {
			target: { value: "dev@100.64.0.9" },
		});
		fireEvent.click(within(await routes()).getByRole("button", { name: "Add route" }));
		await waitFor(async () =>
			expect(await hosts()).toEqual(["dev@devbox.lan", "dev@devbox", "dev@100.64.0.9"])
		);

		fireEvent.click(
			within(await routes()).getByRole("button", { name: "Remove dev@devbox.lan" })
		);
		await waitFor(async () => expect(await hosts()).toEqual(["dev@devbox", "dev@100.64.0.9"]));
	});

	it("says why the app turned a host down", async () => {
		renderDevice("link-devbox");
		fireEvent.change(within(await routes()).getByRole("textbox", { name: "SSH host" }), {
			target: { value: "-oProxyCommand=x" },
		});
		fireEvent.click(within(await routes()).getByRole("button", { name: "Add route" }));
		expect(
			await within(await routes()).findByText('"-oProxyCommand=x" isn\'t an SSH host')
		).toBeVisible();
	});

	it("has no routes for this Mac", async () => {
		renderDevice("link-mac");
		await screen.findByRole("button", { name: "Settings for Mac" });
		expect(screen.queryByRole("region", { name: /^Routes to/ })).toBeNull();
	});
});

describe("the device's maxd version", () => {
	it("notes a maxd newer than the app", async () => {
		renderDevice("link-devbox", { protocol: PROTOCOL_VERSION + 1 });
		expect(
			await screen.findByText(
				"maxd on devbox is newer than this app; update the app for its new features"
			)
		).toBeInTheDocument();
	});

	it("notes a maxd older than the app", async () => {
		renderDevice("link-devbox", { protocol: PROTOCOL_VERSION - 1 });
		expect(
			await screen.findByText(
				"maxd on devbox is older than this app; update it for the app's new features"
			)
		).toBeInTheDocument();
	});

	it("says nothing when they match", async () => {
		renderDevice("link-devbox", { protocol: PROTOCOL_VERSION });
		await restart();
		expect(screen.queryByText(/than this app/)).toBeNull();
	});
});
