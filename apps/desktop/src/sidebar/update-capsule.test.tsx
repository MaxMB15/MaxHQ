// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import { DEVBOX_BUILD } from "@maxhq/shared/data/mock-view";
import type { FleetProgress } from "@maxhq/shared/fleet/wire";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

const renderSidebar = () => {
	const source = mockSource({ now: () => NOW, tick: false });
	const ui = createUiStore();
	ui.getState().setMode("devices");
	return { source, ui };
};

const show = ({ source, ui }: ReturnType<typeof renderSidebar>) =>
	render(
		<FixedNow value={NOW}>
			<App source={source} ui={ui} />
		</FixedNow>
	);

// An updateFleet whose steps the test sends, and which ends when it says.
const driven = (source: ReturnType<typeof renderSidebar>["source"]) => {
	let send: (progress: FleetProgress) => void = () => {};
	let finish: () => void = () => {};
	const updateFleet = vi.spyOn(source, "updateFleet").mockImplementation(
		(_devices, _tools, _unsigned, onProgress) =>
			new Promise((resolve) => {
				send = onProgress;
				finish = () => resolve({ status: "ok", data: null });
			})
	);
	return {
		updateFleet,
		send: (progress: FleetProgress) => act(() => send(progress)),
		finish: () => act(async () => finish()),
	};
};

describe("the Update everything capsule", () => {
	it("offers the CLIs MaxHQ may update, with the app built from source", async () => {
		show(renderSidebar());
		fireEvent.click(await screen.findByRole("button", { name: "Update 1 CLI" }));
		const popup = await screen.findByRole("dialog");
		expect(within(popup).getByText("built from source")).toBeVisible();
		expect(within(popup).getByText("Claude in mhq-maxhq")).toBeVisible();
	});

	it("counts a device's maxd once the app carries a newer build", async () => {
		const setup = renderSidebar();
		vi.spyOn(setup.source, "appInfo").mockResolvedValue({
			version: "0.4.0",
			protocol_version: 1,
			bundled: [
				{ target: "x86_64-unknown-linux-musl", build: `${DEVBOX_BUILD}0`, signed: true },
			],
		});
		show(setup);
		fireEvent.click(await screen.findByRole("button", { name: "Update 1 device and 1 CLI" }));
		const popup = await screen.findByRole("dialog");
		expect(within(popup).getByText("maxd, Claude in mhq-maxhq")).toBeVisible();
		expect(within(popup).getByText("version 0.4.0")).toBeVisible();
		expect(within(popup).queryByText("doesn't check signatures")).toBeNull();
	});

	it("sends a signed build without asking", async () => {
		const setup = renderSidebar();
		vi.spyOn(setup.source, "appInfo").mockResolvedValue({
			version: "0.4.0",
			protocol_version: 1,
			bundled: [
				{ target: "x86_64-unknown-linux-musl", build: `${DEVBOX_BUILD}0`, signed: true },
			],
		});
		const fleet = driven(setup.source);
		show(setup);
		await screen.findByRole("button", { name: "Update 1 device and 1 CLI" });
		fireEvent.click(screen.getByRole("button", { name: "Update everything" }));
		await waitFor(() =>
			expect(fleet.updateFleet).toHaveBeenCalledWith(
				["dev-devbox"],
				true,
				false,
				expect.any(Function)
			)
		);
		expect(screen.queryByRole("alertdialog")).toBeNull();
	});

	it("asks before it sends a maxd no release signed, and sends nothing on Cancel", async () => {
		const setup = renderSidebar();
		vi.spyOn(setup.source, "appInfo").mockResolvedValue({
			version: "0.4.0",
			protocol_version: 1,
			bundled: [
				{ target: "x86_64-unknown-linux-musl", build: `${DEVBOX_BUILD}0`, signed: false },
			],
		});
		const fleet = driven(setup.source);
		show(setup);
		await screen.findByRole("button", { name: "Update 1 device and 1 CLI" });
		fireEvent.click(screen.getByRole("button", { name: "Update everything" }));
		const ask = await screen.findByRole("alertdialog");
		expect(ask).toHaveTextContent(
			"The devices will run a maxd no release signed, built on this Mac."
		);
		fireEvent.click(within(ask).getByRole("button", { name: "Cancel" }));
		await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
		expect(fleet.updateFleet).not.toHaveBeenCalled();

		fireEvent.click(screen.getByRole("button", { name: "Update everything" }));
		fireEvent.click(
			within(await screen.findByRole("alertdialog")).getByRole("button", {
				name: "Install anyway",
			})
		);
		await waitFor(() =>
			expect(fleet.updateFleet).toHaveBeenCalledWith(
				["dev-devbox"],
				true,
				true,
				expect.any(Function)
			)
		);
		// A retry keeps your say.
		fleet.send({ device: "dev-devbox", status: { status: "failed", reason: "disk full" } });
		await fleet.finish();
		const capsule = await screen.findByRole("status", { name: "Update everything" });
		fireEvent.click(within(capsule).getByRole("button", { name: "Retry" }));
		expect(fleet.updateFleet).toHaveBeenLastCalledWith(
			["dev-devbox"],
			true,
			true,
			expect.any(Function)
		);
	});

	it("follows the run and closes once every device is done", async () => {
		const setup = renderSidebar();
		const fleet = driven(setup.source);
		show(setup);
		fireEvent.click(await screen.findByRole("button", { name: "Update everything" }));
		expect(fleet.updateFleet).toHaveBeenCalledWith(
			["dev-devbox"],
			true,
			false,
			expect.any(Function)
		);
		fleet.send({ device: "dev-devbox", status: { status: "uploading" } });
		const capsule = await screen.findByRole("status", { name: "Update everything" });
		expect(capsule).toHaveTextContent("0 of 1 updated");
		fleet.send({ device: "dev-devbox", status: { status: "done" } });
		await fleet.finish();
		await waitFor(() =>
			expect(screen.queryByRole("status", { name: "Update everything" })).toBeNull()
		);
	});

	it("retries a device that failed", async () => {
		const setup = renderSidebar();
		const fleet = driven(setup.source);
		show(setup);
		fireEvent.click(await screen.findByRole("button", { name: "Update everything" }));
		fleet.send({ device: "dev-devbox", status: { status: "failed", reason: "disk full" } });
		await fleet.finish();
		const capsule = await screen.findByRole("status", { name: "Update everything" });
		expect(capsule).toHaveTextContent("devbox failed");
		fireEvent.click(within(capsule).getByRole("button", { name: "Retry" }));
		expect(fleet.updateFleet).toHaveBeenCalledTimes(2);
		expect(fleet.updateFleet).toHaveBeenLastCalledWith(
			["dev-devbox"],
			true,
			false,
			expect.any(Function)
		);
		expect(capsule).toHaveTextContent("0 of 1 updated");
	});

	it("dismisses a run where a device was skipped", async () => {
		const setup = renderSidebar();
		const fleet = driven(setup.source);
		show(setup);
		fireEvent.click(await screen.findByRole("button", { name: "Update everything" }));
		fleet.send({ device: "dev-devbox", status: { status: "skipped", reason: "winding down" } });
		await fleet.finish();
		const capsule = await screen.findByRole("status", { name: "Update everything" });
		fireEvent.click(within(capsule).getByRole("button", { name: "Dismiss" }));
		await waitFor(() =>
			expect(screen.queryByRole("status", { name: "Update everything" })).toBeNull()
		);
		expect(await screen.findByRole("button", { name: "Update 1 CLI" })).toBeVisible();
	});
});
