// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import type { DataSource } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import type { Entity, ManagedWorkspace } from "@maxhq/shared/bindings";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

// The mock source with devbox's workspace changed.
const withWorkspace = (source: DataSource, change: Partial<ManagedWorkspace>): DataSource => ({
	...source,
	snapshot: async () => {
		const snapshot = await source.snapshot();
		const entities = snapshot.entities.map((e): Entity =>
			e.entity === "workspace" && e.id === "ws-mhq-maxhq" && e.managed
				? { ...e, managed: { ...e.managed, ...change } }
				: e
		);
		return { ...snapshot, entities };
	},
});

// The mock source with devbox's `maxd` saying it makes no workspaces.
const makingNone = (source: DataSource): DataSource => ({
	...source,
	snapshot: async () => {
		const snapshot = await source.snapshot();
		const entities = snapshot.entities.map((e): Entity =>
			e.entity === "device_link" && e.device === "dev-devbox" && e.capabilities
				? { ...e, capabilities: { ...e.capabilities, workspaces: false } }
				: e
		);
		return { ...snapshot, entities };
	},
});

const renderDevice = (
	link: string,
	{
		scenario = "connected",
		change,
		noWorkspaces = false,
	}: { scenario?: string; change?: Partial<ManagedWorkspace>; noWorkspaces?: boolean } = {}
) => {
	const mock = mockSource({ now: () => NOW, tick: false, scenario });
	const changed = change ? withWorkspace(mock, change) : mock;
	const source = noWorkspaces ? makingNone(changed) : changed;
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

const workspaces = () => screen.findByRole("region", { name: "Workspaces on devbox" });
const card = async () =>
	within(await workspaces()).getByRole("group", { name: "Workspace mhq-maxhq" });

const pick = async (label: string, text: string) => {
	fireEvent.click(await screen.findByRole("combobox", { name: new RegExp(`^${label},`) }));
	const found = (await screen.findAllByRole("option")).find((o) => o.textContent?.includes(text));
	if (!found) throw new Error(`No option "${text}"`);
	fireEvent.pointerDown(found, { pointerType: "mouse" });
	fireEvent.click(found);
};

afterEach(() => {
	vi.useRealTimers();
});

describe("new workspace", () => {
	it("sends the spec and follows the workspace step by step", async () => {
		vi.useFakeTimers({ shouldAdvanceTime: true });
		const { source } = renderDevice("link-devbox");
		const create = vi.spyOn(source, "createWorkspace");
		fireEvent.click(within(await workspaces()).getByRole("button", { name: "New workspace" }));
		const dialog = await screen.findByRole("dialog");
		expect(within(dialog).getByText("Pick a project.")).toBeInTheDocument();
		expect(within(dialog).getByRole("button", { name: "Create" })).toBeDisabled();

		await pick("Project", "Comet");
		expect(
			within(dialog).getByRole("combobox", { name: "Device, devbox" })
		).toBeInTheDocument();
		fireEvent.click(within(dialog).getByRole("button", { name: "Create" }));

		await waitFor(() =>
			expect(create).toHaveBeenCalledWith({
				project: "proj-comet",
				device: "dev-devbox",
				name: null,
				start_at_boot: true,
			})
		);
		expect(await within(dialog).findByText("Making mhq-comet")).toBeInTheDocument();
		expect(within(dialog).getByText("Making the container")).toBeInTheDocument();
		expect(within(dialog).getByText("4 more steps")).toBeInTheDocument();

		await act(() => vi.advanceTimersByTimeAsync(5 * 700));
		expect(await within(dialog).findByText("mhq-comet is ready.")).toBeInTheDocument();
		const steps = within(within(dialog).getByRole("list", { name: "Steps" })).getAllByRole(
			"listitem"
		);
		expect(steps.map((s) => s.textContent)).toEqual([
			"Making the container",
			"Checking what it can reach",
			"Installing the tools",
			"Cloning the repo",
			"Checking it can push",
		]);
		expect(
			within(dialog).getByText(
				"Can't reach your LAN or tailnet. The public internet stays open. Checked at 14:48"
			)
		).toBeInTheDocument();
	});

	it("sends the name and boot choice you give it", async () => {
		const { source } = renderDevice("link-devbox");
		const create = vi.spyOn(source, "createWorkspace");
		fireEvent.click(within(await workspaces()).getByRole("button", { name: "New workspace" }));
		const dialog = await screen.findByRole("dialog");
		await pick("Project", "Comet");
		fireEvent.change(within(dialog).getByRole("textbox", { name: "Container name" }), {
			target: { value: "com-2" },
		});
		fireEvent.click(within(dialog).getByRole("switch", { name: "Start at boot" }));
		fireEvent.click(within(dialog).getByRole("button", { name: "Create" }));

		await waitFor(() =>
			expect(create).toHaveBeenCalledWith({
				project: "proj-comet",
				device: "dev-devbox",
				name: "com-2",
				start_at_boot: false,
			})
		);
		expect(await within(dialog).findByText("Making mhq-com-2")).toBeInTheDocument();
	});

	it("gives devbox this Mac's git identity for the project before it makes the container", async () => {
		const { source } = renderDevice("link-devbox");
		vi.spyOn(source, "gitIdentity").mockResolvedValue({
			name: "Sam Placeholder",
			email: "sam@example.com",
		});
		const save = vi.spyOn(source, "setProjectSettings");
		const create = vi.spyOn(source, "createWorkspace");
		fireEvent.click(within(await workspaces()).getByRole("button", { name: "New workspace" }));
		const dialog = await screen.findByRole("dialog");
		await pick("Project", "Comet");
		fireEvent.click(within(dialog).getByRole("button", { name: "Create" }));

		await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
		expect(save).toHaveBeenCalledExactlyOnceWith({
			project: "proj-comet",
			device: "dev-devbox",
			resume_on_restart: true,
			ignored_ports: [],
			git_author: { name: "Sam Placeholder", email: "sam@example.com" },
		});
		expect(save.mock.invocationCallOrder[0]).toBeLessThan(create.mock.invocationCallOrder[0]!);
	});

	it("keeps the settings devbox has for the project", async () => {
		const { source } = renderDevice("link-devbox");
		const save = vi.spyOn(source, "setProjectSettings");
		const create = vi.spyOn(source, "createWorkspace");
		fireEvent.click(within(await workspaces()).getByRole("button", { name: "New workspace" }));
		const dialog = await screen.findByRole("dialog");
		await pick("Project", "MaxHQ");
		fireEvent.change(within(dialog).getByRole("textbox", { name: "Container name" }), {
			target: { value: "maxhq-2" },
		});
		fireEvent.click(within(dialog).getByRole("button", { name: "Create" }));

		await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
		expect(save).not.toHaveBeenCalled();
	});

	it("still makes the container when git has no identity", async () => {
		const { source } = renderDevice("link-devbox");
		vi.spyOn(source, "gitIdentity").mockResolvedValue({ name: null, email: null });
		const save = vi.spyOn(source, "setProjectSettings");
		const create = vi.spyOn(source, "createWorkspace");
		fireEvent.click(within(await workspaces()).getByRole("button", { name: "New workspace" }));
		const dialog = await screen.findByRole("dialog");
		await pick("Project", "Comet");
		fireEvent.click(within(dialog).getByRole("button", { name: "Create" }));

		expect(await within(dialog).findByText("Making mhq-comet")).toBeInTheDocument();
		expect(create).toHaveBeenCalledTimes(1);
		expect(save).not.toHaveBeenCalled();
	});

	it("offers none on a device whose maxd makes no workspaces", async () => {
		renderDevice("link-devbox", { noWorkspaces: true });
		const section = await workspaces();
		await within(section).findByText("devbox doesn't make workspaces now");
		expect(within(section).queryByRole("button", { name: "New workspace" })).toBeNull();
		expect(
			within(section).getByRole("group", { name: "Workspace mhq-maxhq" })
		).toBeInTheDocument();
	});

	it("won't make a second container with the same name", async () => {
		const { source } = renderDevice("link-devbox");
		const create = vi.spyOn(source, "createWorkspace");
		fireEvent.click(within(await workspaces()).getByRole("button", { name: "New workspace" }));
		const dialog = await screen.findByRole("dialog");
		await pick("Project", "MaxHQ");
		expect(within(dialog).getByText("mhq-maxhq already exists on devbox.")).toBeInTheDocument();
		expect(within(dialog).getByRole("button", { name: "Create" })).toBeDisabled();
		expect(create).not.toHaveBeenCalled();
	});
});

describe("workspace card", () => {
	it("names what answered when the egress check fails", async () => {
		renderDevice("link-devbox", {
			change: {
				egress: {
					state: "failed",
					at: new Date(2026, 9, 25, 14, 30).getTime(),
					reached: ["192.168.1.1", "devbox.local"],
				},
			},
		});
		expect(
			within(await card()).getByText(
				"Egress check failed at 14:30. 192.168.1.1 and devbox.local answered, so agents won't start until a check passes"
			)
		).toBeInTheDocument();
	});

	it("shows storage locked as purple and offers only Delete", async () => {
		renderDevice("link-devbox", { change: { state: { state: "storage_locked" } } });
		const group = await card();
		const dot = within(group).getAllByRole("img", {
			name: "Storage locked. Unlock the device's storage, then start it",
		})[0];
		expect(dot).toHaveAttribute("data-look", "agent_down");
		expect(
			within(group).getByText("Storage locked. Unlock the device's storage, then start it", {
				selector: "p",
			})
		).toHaveClass("text-status-linked");
		expect(
			within(group)
				.getAllByRole("button")
				.map((b) => b.textContent)
		).toEqual(["Delete"]);
	});

	it("stops and starts a workspace", async () => {
		vi.useFakeTimers({ shouldAdvanceTime: true });
		const { source } = renderDevice("link-devbox");
		const stop = vi.spyOn(source, "stopWorkspace");
		const start = vi.spyOn(source, "startWorkspace");
		fireEvent.click(within(await card()).getByRole("button", { name: "Stop" }));
		expect(stop).toHaveBeenCalledWith("ws-mhq-maxhq");
		await act(() => vi.advanceTimersByTimeAsync(700));
		fireEvent.click(await within(await card()).findByRole("button", { name: "Start" }));
		expect(start).toHaveBeenCalledWith("ws-mhq-maxhq");
	});

	it("asks before a delete, then takes a second click to discard unpushed work", async () => {
		const { source } = renderDevice("link-devbox");
		const remove = vi
			.spyOn(source, "deleteWorkspace")
			.mockResolvedValueOnce({
				status: "ok",
				data: { status: "refused", reason: "mhq-maxhq has 2 commits that aren't pushed" },
			})
			.mockResolvedValueOnce({ status: "ok", data: { status: "done" } });
		const group = await card();
		fireEvent.click(within(group).getByRole("button", { name: "Delete" }));
		expect(remove).not.toHaveBeenCalled();

		const confirm = within(group).getByRole("group", { name: "Delete mhq-maxhq" });
		fireEvent.click(within(confirm).getByRole("button", { name: "Delete" }));
		expect(remove).toHaveBeenLastCalledWith("ws-mhq-maxhq", false);

		const alert = await within(group).findByRole("alert");
		expect(alert).toHaveTextContent("mhq-maxhq has 2 commits that aren't pushed.");
		expect(alert).toHaveTextContent(
			"Push it first to keep it. Deleting now throws that work away for good."
		);
		fireEvent.click(within(alert).getByRole("button", { name: "Discard and delete" }));
		expect(remove).toHaveBeenLastCalledWith("ws-mhq-maxhq", true);
		await waitFor(() => expect(within(group).queryByRole("alert")).not.toBeInTheDocument());
		expect(remove).toHaveBeenCalledTimes(2);
	});

	it("keeps the workspace when you change your mind", async () => {
		const { source } = renderDevice("link-devbox");
		const remove = vi.spyOn(source, "deleteWorkspace");
		const group = await card();
		fireEvent.click(within(group).getByRole("button", { name: "Delete" }));
		fireEvent.click(within(group).getByRole("button", { name: "Keep it" }));
		expect(within(group).queryByRole("group", { name: "Delete mhq-maxhq" })).toBeNull();
		expect(remove).not.toHaveBeenCalled();
	});

	it("needs the device connected to change it", async () => {
		renderDevice("link-devbox", { scenario: "tailscale_off" });
		const group = await card();
		expect(within(group).queryByRole("button")).toBeNull();
		expect(
			within(group).getByText("Connect to devbox to start, stop or delete it.")
		).toBeInTheDocument();
		expect(
			within(await workspaces()).getByRole("button", { name: "New workspace" })
		).toBeDisabled();
	});
});
