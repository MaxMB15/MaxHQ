// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ROOT_FIX } from "@maxhq/shared/data/mock-hosts";
import { DEVBOX, MAC } from "@maxhq/shared/data/mock-view";

import { DeviceSetup } from "./device-setup";
import { renderSection } from "./device-testing";

const terminal = vi.hoisted(() => ({
	openRootFix: vi.fn(async () => null),
	opensTerminal: vi.fn(() => true),
}));
vi.mock("@/native/terminal", () => terminal);

describe("a device's Setup", () => {
	it("shows each item with its state, problems first in each group", async () => {
		renderSection(DeviceSetup, DEVBOX, "devbox");
		const limits = within(await screen.findByRole("group", { name: "Limits" }));
		expect(limits.getByText("Compressed swap")).toBeInTheDocument();
		expect(limits.getByText("off, should be on")).toBeInTheDocument();
		expect(limits.getAllByRole("img", { name: "Needs a fix" }).length).toBeGreaterThan(0);
	});

	it("shows the root command once, over ssh, with Copy and Open in Terminal", async () => {
		renderSection(DeviceSetup, DEVBOX, "devbox");
		const root = within(await screen.findByRole("group", { name: "Root command" }));
		expect(root.getByText(/^ssh -t /)).toHaveTextContent(`'${ROOT_FIX}'`);
		expect(screen.getAllByRole("group", { name: "Root command" })).toHaveLength(1);
		expect(root.getByRole("button", { name: "Copy command" })).toBeInTheDocument();
		fireEvent.click(root.getByRole("button", { name: "Open in Terminal" }));
		// The page names only the item, and the backend reads the command
		// from the device's report.
		expect(terminal.openRootFix).toHaveBeenCalledWith(DEVBOX, expect.any(String));
	});

	it("applies only the items maxd can fix itself, then the Mac is set up", async () => {
		const { source } = renderSection(DeviceSetup, MAC, "Mac");
		const apply = vi.spyOn(source, "applyHost");
		expect(
			await screen.findByText("No spec file, so the defaults", { exact: false })
		).toBeInTheDocument();
		expect(screen.queryByRole("group", { name: "Root command" })).toBeNull();
		fireEvent.click(screen.getByRole("button", { name: "Apply" }));
		expect(apply).toHaveBeenCalledWith(MAC, ["tools.user.gh"]);
		expect(await screen.findByText("Mac is set up")).toBeInTheDocument();
		expect(screen.queryByRole("button", { name: "Apply" })).toBeNull();
	});

	it("checks again on request", async () => {
		const { source } = renderSection(DeviceSetup, MAC, "Mac");
		const check = vi.spyOn(source, "checkHost");
		fireEvent.click(await screen.findByRole("button", { name: "Check again" }));
		expect(check).toHaveBeenCalledWith(MAC);
	});
});
