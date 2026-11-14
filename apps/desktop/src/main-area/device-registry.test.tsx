// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { DEVBOX, MAC } from "@maxhq/shared/data/mock-view";

import { DeviceRegistry } from "./device-registry";
import { renderSection } from "./device-testing";

const section = async (name: string) =>
	within(await screen.findByRole("region", { name: `ACP registry for ${name}` }));

describe("the ACP registry", () => {
	it("searches the agents and installs or updates them", async () => {
		const { source } = renderSection(DeviceRegistry, MAC, "Mac");
		const mac = await section("Mac");
		const install = vi.spyOn(source, "installAcpAgent");
		expect(await mac.findByText("Not available for this device")).toBeInTheDocument();
		expect(mac.queryByRole("button", { name: /Desktop agent/ })).toBeNull();
		fireEvent.click(mac.getByRole("button", { name: /^Update / }));
		expect(install).toHaveBeenCalledWith(
			MAC,
			"goose",
			expect.any(String),
			{ type: "host" },
			false
		);

		fireEvent.change(mac.getByRole("searchbox", { name: "Search the registry" }), {
			target: { value: "fast" },
		});
		expect(mac.getByText("fast-agent")).toBeInTheDocument();
		expect(mac.queryByText("Kiro")).toBeNull();
		fireEvent.change(mac.getByRole("searchbox"), { target: { value: "nothing like it" } });
		expect(mac.getByText("No agent matches.")).toBeInTheDocument();
	});

	it("asks before it installs a binary with no checksum", async () => {
		const { source } = renderSection(DeviceRegistry, MAC, "Mac");
		const mac = await section("Mac");
		const install = vi.spyOn(source, "installAcpAgent");
		fireEvent.click(await mac.findByRole("button", { name: "Install Kiro" }));
		expect(install).not.toHaveBeenCalled();
		expect(mac.getByRole("alert")).toHaveTextContent(
			"The registry gives no checksum for Kiro 0.4.1"
		);
		fireEvent.click(mac.getByRole("button", { name: "Cancel" }));
		expect(mac.queryByRole("alert")).toBeNull();
		fireEvent.click(mac.getByRole("button", { name: "Install Kiro" }));
		fireEvent.click(mac.getByRole("button", { name: "Install anyway" }));
		expect(install).toHaveBeenCalledWith(MAC, "kiro", "0.4.1", { type: "host" }, true);
	});

	it("says when the registry couldn't be reached", async () => {
		renderSection(DeviceRegistry, DEVBOX, "devbox");
		const devbox = await section("devbox");
		expect(devbox.getByText(/Couldn't reach the registry/)).toBeInTheDocument();
		expect(devbox.getByText("The registry lists no agents.")).toBeInTheDocument();
	});
});
