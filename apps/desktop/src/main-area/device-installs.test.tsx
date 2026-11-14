// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { DEVBOX, MAC } from "@maxhq/shared/data/mock-view";

import { DeviceInstalls } from "./device-installs";
import { renderSection } from "./device-testing";

const host = async (name: string) =>
	within(await screen.findByRole("group", { name: `On ${name}` }));

describe("the tools a device installs", () => {
	it("shows each install's phase and bytes, and the helpers it lacks", async () => {
		const { source } = renderSection(DeviceInstalls, MAC, "Mac");
		const mac = await host("Mac");
		const bar = await mac.findByRole("progressbar", { name: "Installing Java" });
		expect(bar).toHaveAttribute("aria-valuenow", "63");
		expect(mac.getByText(/^Downloading .* of /)).toBeInTheDocument();
		expect(mac.getByRole("button", { name: "Read licences" })).toBeInTheDocument();

		const install = vi.spyOn(source, "installTool");
		fireEvent.click(mac.getByRole("button", { name: "Install GitHub CLI" }));
		expect(install).toHaveBeenCalledWith(MAC, { type: "host" }, { type: "helper", tool: "gh" });
		// The Android SDK is listed already, so it isn't offered twice.
		expect(mac.queryByRole("button", { name: "Install Android SDK" })).toBeNull();
	});

	it("removes a tool MaxHQ installed once you confirm", async () => {
		const { source } = renderSection(DeviceInstalls, MAC, "Mac");
		const mac = await host("Mac");
		const remove = vi.spyOn(source, "removeTool");
		const row = (await mac.findByText("Node.js")).closest("div.flex")!.parentElement!;
		fireEvent.click(within(row).getByRole("button", { name: "Remove" }));
		expect(remove).not.toHaveBeenCalled();
		fireEvent.click(within(row).getByRole("button", { name: "Remove Node.js" }));
		expect(remove).toHaveBeenCalledWith(
			MAC,
			{ type: "host" },
			{ type: "helper", tool: "node" }
		);
	});

	it("tries a failed install again, and groups a workspace's tools under it", async () => {
		const { source } = renderSection(DeviceInstalls, DEVBOX, "devbox");
		const devbox = await host("devbox");
		const install = vi.spyOn(source, "installTool");
		fireEvent.click(devbox.getByRole("button", { name: "Try installing uv again" }));
		expect(install).toHaveBeenCalledWith(
			DEVBOX,
			{ type: "host" },
			{ type: "helper", tool: "uv" }
		);
		// No Android SDK off the Mac.
		expect(devbox.queryByRole("button", { name: "Install Android SDK" })).toBeNull();
		const workspace = screen.getByRole("group", { name: /^In / });
		expect(within(workspace).getByText("Checking the download")).toBeInTheDocument();
		expect(within(workspace).getByRole("progressbar")).not.toHaveAttribute("aria-valuenow");
	});

	it("opens the licences an install waits on", async () => {
		renderSection(DeviceInstalls, MAC, "Mac");
		fireEvent.click((await host("Mac")).getByRole("button", { name: "Read licences" }));
		const dialog = await screen.findByRole("dialog", { name: "Licences to accept" });
		expect(within(dialog).getByText(/Terms and conditions/)).toBeInTheDocument();
	});
});
