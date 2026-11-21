// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import { openDeviceTab } from "@/test/settings";
import type { Entity } from "@maxhq/shared/bindings";
import type { DataSource } from "@maxhq/shared/data/source";
import { IOS_LEASE, MAC_IOS, MAC_MACOS } from "@maxhq/shared/data/mock-test-hosts";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

import { SCREEN_EVERY } from "./lease-view";

afterEach(() => vi.useRealTimers());

// The mock source with each entity passed through `edit`.
const editing = (source: DataSource, edit: (e: Entity) => Entity): DataSource => ({
	...source,
	snapshot: async () => {
		const snapshot = await source.snapshot();
		return { ...snapshot, entities: snapshot.entities.map(edit) };
	},
});

const renderApp = ({
	scenario = "connected",
	edit,
	setup,
}: {
	scenario?: string;
	edit?: (e: Entity) => Entity;
	setup?: (ui: ReturnType<typeof createUiStore>) => void;
} = {}) => {
	const mock = mockSource({ now: () => NOW, tick: false, scenario });
	const source = edit ? editing(mock, edit) : mock;
	const ui = createUiStore();
	setup?.(ui);
	render(
		<FixedNow value={NOW}>
			<App source={source} ui={ui} />
		</FixedNow>
	);
	return { source: mock };
};

// A device's Test machines card, on its tab in Settings.
const openCard = async (name: string) => {
	if (!screen.queryByRole("dialog", { name: "Settings" })) {
		await screen.findByRole("button", { name: "Settings" });
	}
	const dialog = await openDeviceTab(name);
	return within(await within(dialog).findByRole("region", { name: `Test machines on ${name}` }));
};

const renderDevice = (link: string, scenario = "connected") =>
	renderApp({
		scenario,
		setup: (ui) => {
			ui.getState().setMode("devices");
			ui.getState().selectLink(link);
		},
	});

const openAgent = async (title: RegExp) => {
	const nav = await screen.findByRole("navigation", { name: "Agents" });
	const maxhq = await within(nav).findByRole("region", { name: "MaxHQ" });
	fireEvent.click(within(maxhq).getByRole("option", { name: title }));
	await screen.findByRole("heading", { level: 1, name: title });
};

const noTestHosts = (e: Entity): Entity =>
	e.entity === "device_link"
		? { ...e, capabilities: { ...e.capabilities!, test_hosts: false } }
		: e;

describe("a device's Test machines card", () => {
	it("lists each kind the device could host, with its lease limits", async () => {
		renderApp();
		const mac = await openCard("Mac");
		expect(mac.getByRole("checkbox", { name: "Host iOS Simulator on Mac" })).toBeChecked();
		expect(mac.getByText("Suggested, its VM is built")).toBeInTheDocument();
		expect(mac.getByText("Suggested, runs the emulator best")).toBeInTheDocument();
		// The Mac VM hosts iOS and macOS, so it shows under each.
		expect(mac.getAllByRole("group", { name: "Mac VM on Mac" })[0]).toHaveTextContent(
			"60 GB disk, 8 GB memory"
		);
		expect(mac.getByRole("switch", { name: "Use Mac's own Simulator" })).not.toBeChecked();
		expect(mac.getByText(/Apps it runs can read your home folder/)).toBeInTheDocument();
		expect(mac.queryByRole("checkbox", { name: /Linux/ })).toBeNull();
		expect(mac.getByRole("combobox", { name: "Lease length, 30 min" })).toBeInTheDocument();
		expect(mac.getByRole("combobox", { name: "macOS VMs at once, 1" })).toBeInTheDocument();
		expect(
			mac.getByText(/Only agents on a device lease its test machines/)
		).toBeInTheDocument();

		const devbox = await openCard("devbox");
		expect(
			devbox.getByRole("checkbox", { name: "Host Android emulator on devbox" })
		).not.toBeChecked();
		expect(devbox.queryByText(/^Suggested/)).toBeNull();
	});

	it("ticks a kind and shows what it found there", async () => {
		const { source } = renderApp();
		const save = vi.spyOn(source, "setDeviceSettings");
		const devbox = await openCard("devbox");
		fireEvent.click(devbox.getByRole("checkbox", { name: "Host Android emulator on devbox" }));
		await waitFor(() => expect(save).toHaveBeenCalled());
		expect(save.mock.calls[0]![0]).toMatchObject({
			device: "dev-devbox",
			test_hosts: { kinds: ["android_emulator", "linux"] },
		});
		expect(await devbox.findByText(/The Android SDK isn't installed/)).toBeInTheDocument();
		expect(devbox.getByRole("button", { name: "Check devbox again" })).toBeInTheDocument();
	});

	it("builds a host that's off and checks the device again", async () => {
		const { source } = renderApp();
		const build = vi.spyOn(source, "buildTestHost");
		const check = vi.spyOn(source, "checkTestHosts");
		const mac = await openCard("Mac");
		fireEvent.click(mac.getByRole("button", { name: "Build Mac VM on Mac" }));
		expect(build).toHaveBeenCalledWith("dev-mac", MAC_MACOS);
		fireEvent.click(mac.getByRole("button", { name: "Check Mac again" }));
		expect(check).toHaveBeenCalledWith("dev-mac");
	});

	it("asks before resetting a leased host", async () => {
		const { source } = renderApp();
		const reset = vi.spyOn(source, "resetTestHost");
		const mac = await openCard("Mac");
		const vm = within(mac.getAllByRole("group", { name: "Mac VM on Mac" })[0]!);
		fireEvent.click(vm.getByRole("button", { name: "Reset" }));
		expect(vm.getByText("This ends its lease.")).toBeInTheDocument();
		expect(reset).not.toHaveBeenCalled();
		fireEvent.click(vm.getByRole("button", { name: "Reset" }));
		expect(reset).toHaveBeenCalledWith("dev-mac", MAC_IOS);
	});

	it("turns on the Mac's own Simulator", async () => {
		const { source } = renderApp();
		const save = vi.spyOn(source, "setDeviceSettings");
		const mac = await openCard("Mac");
		fireEvent.click(mac.getByRole("switch", { name: "Use Mac's own Simulator" }));
		await waitFor(() =>
			expect(save).toHaveBeenCalledWith(
				expect.objectContaining({
					device: "dev-mac",
					test_hosts: expect.objectContaining({ mac_host: true }),
				})
			)
		);
	});

	it("saves a lease limit", async () => {
		const { source } = renderApp();
		const save = vi.spyOn(source, "setDeviceSettings");
		const mac = await openCard("Mac");
		fireEvent.click(mac.getByRole("combobox", { name: "Ends when idle, 10 min" }));
		const twenty = (await screen.findAllByRole("option")).find(
			(o) => o.textContent === "20 min"
		)!;
		fireEvent.pointerDown(twenty, { pointerType: "mouse" });
		fireEvent.click(twenty);
		await waitFor(() =>
			expect(save).toHaveBeenCalledWith(
				expect.objectContaining({
					test_hosts: expect.objectContaining({ idle_minutes: 20 }),
				})
			)
		);
	});

	it("can't change a device that doesn't answer", async () => {
		renderApp({ scenario: "peer_offline" });
		const devbox = await openCard("devbox");
		expect(devbox.getByRole("checkbox", { name: "Host Linux on devbox" })).toHaveAttribute(
			"aria-disabled",
			"true"
		);
		expect(devbox.getAllByText("Connect to devbox to change it.").length).toBeGreaterThan(0);
	});

	it("is left out when the device doesn't offer test hosts", async () => {
		renderApp({ edit: noTestHosts });
		await screen.findByRole("button", { name: "Settings" });
		const dialog = await openDeviceTab("Mac");
		await within(dialog).findByRole("region", { name: "Connection" });
		expect(within(dialog).queryByRole("region", { name: "Test machines on Mac" })).toBeNull();
	});
});

describe("a device's leases", () => {
	it("shows on the device's page", async () => {
		renderDevice("link-mac");
		const leases = within(await screen.findByRole("region", { name: "Leases on Mac" }));
		expect(leases.getByText("iOS Simulator for Build a signed bundle")).toBeInTheDocument();
		expect(leases.getByText("Ready, 8 min left, Mac VM")).toBeInTheDocument();
		expect(leases.getByText(/start an agent on Mac/)).toBeInTheDocument();
		expect(screen.queryByRole("region", { name: "Test machines on Mac" })).toBeNull();
	});

	it("ends a lease after asking", async () => {
		const { source } = renderDevice("link-mac");
		const end = vi.spyOn(source, "endLease");
		const leases = within(await screen.findByRole("region", { name: "Leases on Mac" }));
		fireEvent.click(leases.getByRole("button", { name: "End" }));
		expect(leases.getByText("Its holder loses the test machine.")).toBeInTheDocument();
		fireEvent.click(leases.getByRole("button", { name: "End lease" }));
		expect(end).toHaveBeenCalledWith("dev-mac", IOS_LEASE);
		expect(await leases.findByText(/No agent holds a test machine on Mac/)).toBeInTheDocument();
	});

	it("is hidden when the device doesn't offer test hosts", async () => {
		renderApp({
			edit: noTestHosts,
			setup: (ui) => {
				ui.getState().setMode("devices");
				ui.getState().selectLink("link-mac");
			},
		});
		await screen.findByRole("region", { name: "Restart Mac" });
		expect(screen.queryByRole("region", { name: "Leases on Mac" })).toBeNull();
	});
});

describe("a lease", () => {
	it("shows on its holder's header and opens its view", async () => {
		const { source } = renderApp();
		const read = vi.spyOn(source, "readLeaseScreen");
		await openAgent(/Build a signed bundle/);
		fireEvent.click(await screen.findByRole("button", { name: "iOS Simulator, 8 min" }));
		const view = within(await screen.findByRole("dialog", { name: "iOS Simulator lease" }));
		expect(view.getByText(/Held by Build a signed bundle on Mac, Mac VM/)).toBeInTheDocument();
		expect(await view.findByRole("img", { name: "iOS Simulator screen" })).toHaveAttribute(
			"src",
			expect.stringMatching(/^data:image\/png;base64,/)
		);
		expect(read).toHaveBeenCalledWith("dev-mac", IOS_LEASE);
		expect(view.getByText("Taken at 14:48")).toBeInTheDocument();
		expect(view.getByRole("list", { name: "Last commands" })).toHaveTextContent(/xcrun simctl/);
	});

	it("takes the screen again every 2 seconds while it's open", async () => {
		vi.useFakeTimers({ shouldAdvanceTime: true });
		const { source } = renderApp();
		const read = vi.spyOn(source, "readLeaseScreen");
		await openAgent(/Build a signed bundle/);
		fireEvent.click(await screen.findByRole("button", { name: "iOS Simulator, 8 min" }));
		await screen.findByRole("img", { name: "iOS Simulator screen" });
		const before = read.mock.calls.length;
		await act(() => vi.advanceTimersByTimeAsync(SCREEN_EVERY));
		expect(read.mock.calls.length).toBe(before + 1);

		fireEvent.click(screen.getByRole("button", { name: "Close" }));
		await waitFor(() =>
			expect(screen.queryByRole("dialog", { name: "iOS Simulator lease" })).toBeNull()
		);
		const closed = read.mock.calls.length;
		await act(() => vi.advanceTimersByTimeAsync(3 * SCREEN_EVERY));
		expect(read.mock.calls.length).toBe(closed);
	});

	it("ends from its view", async () => {
		const { source } = renderApp();
		const end = vi.spyOn(source, "endLease");
		await openAgent(/Build a signed bundle/);
		fireEvent.click(await screen.findByRole("button", { name: "iOS Simulator, 8 min" }));
		const view = within(await screen.findByRole("dialog", { name: "iOS Simulator lease" }));
		fireEvent.click(view.getByRole("button", { name: "End lease" }));
		fireEvent.click(view.getByRole("button", { name: "End lease" }));
		expect(end).toHaveBeenCalledWith("dev-mac", IOS_LEASE);
		expect(await view.findByText("This lease has ended.")).toBeInTheDocument();
		expect(screen.queryByRole("button", { name: "iOS Simulator, 8 min" })).toBeNull();
	});
});
