// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { renderSection } from "@/main-area/device-testing";
import { DEVBOX, MAC } from "@maxhq/shared/data/mock-view";

import { DeviceMoves } from "./device-moves";

const card = async (source: string) =>
	within(await screen.findByRole("group", { name: `Move of ${source}` }));

describe("the move cards", () => {
	it("shows each step with what it changes, and runs the step it's at", async () => {
		const { source } = renderSection(DeviceMoves, DEVBOX, "devbox");
		const alpha = await card("ws-alpha");
		expect(alpha.getByLabelText("Copy: Waiting for you")).toBeInTheDocument();
		expect(alpha.getByLabelText("Set up: Not yet")).toBeInTheDocument();
		expect(alpha.getByText("Set a 3 GiB soft memory limit")).toBeInTheDocument();

		const run = vi.spyOn(source, "runMoveStep");
		fireEvent.click(alpha.getByRole("button", { name: "Copy" }));
		expect(run).toHaveBeenCalledWith("move-alpha", "copy");
	});

	it("lets any card go on, whatever the others are at", async () => {
		const { source } = renderSection(DeviceMoves, DEVBOX, "devbox");
		const run = vi.spyOn(source, "runMoveStep");

		const delta = await card("ws-delta");
		expect(
			delta.getByText("The guest agent didn't answer within 2 minutes.")
		).toBeInTheDocument();
		fireEvent.click(delta.getByRole("button", { name: "Try again" }));
		expect(run).toHaveBeenCalledWith("move-delta", "converge");

		const bravo = await card("ws-bravo");
		expect(bravo.getByText("ws-bravo changed since the copy.")).toBeInTheDocument();
		fireEvent.click(bravo.getByRole("button", { name: "Copy again" }));
		expect(run).toHaveBeenCalledWith("move-bravo", "copy");

		const echo = await card("ws-echo");
		expect(echo.getByText("Moved back. ws-echo runs again.")).toBeInTheDocument();
		expect(echo.queryByRole("button")).toBeNull();
	});

	it("moves back while the original is kept", async () => {
		const { source } = renderSection(DeviceMoves, DEVBOX, "devbox");
		const charlie = await card("ws-charlie");
		expect(charlie.getByText(/^ws-charlie is kept until /)).toBeInTheDocument();

		const back = vi.spyOn(source, "moveBack");
		fireEvent.click(charlie.getByRole("button", { name: "Move back" }));
		expect(back).toHaveBeenCalledWith("move-charlie");
	});

	it("asks with what's unsaved before it retires the original", async () => {
		const { source } = renderSection(DeviceMoves, DEVBOX, "devbox");
		const charlie = await card("ws-charlie");
		const retire = vi.spyOn(source, "retireMove");
		fireEvent.click(charlie.getByRole("button", { name: "Retire now" }));
		expect(retire).not.toHaveBeenCalled();
		const ask = charlie.getByRole("alert", { name: "Retire ws-charlie" });
		expect(
			within(ask).getByText(/^MaxHQ can't read unsaved work in ws-charlie/)
		).toBeInTheDocument();
		fireEvent.click(within(ask).getByRole("button", { name: "Delete ws-charlie" }));
		expect(retire).toHaveBeenCalledWith("move-charlie");
	});

	it("offers to move a container MaxHQ didn't make and isn't moving", async () => {
		const { source } = renderSection(DeviceMoves, DEVBOX, "devbox");
		const plan = vi.spyOn(source, "planMove");
		fireEvent.click(
			await screen.findByRole("button", { name: "Move ws-example-app into MaxHQ" })
		);
		expect(plan).toHaveBeenCalledWith(DEVBOX, "ws-example-app");
		expect(screen.queryByRole("button", { name: "Move ws-alpha into MaxHQ" })).toBeNull();
		expect(screen.queryByRole("button", { name: /Move ws-mhq-maxhq/ })).toBeNull();
	});

	it("shows nothing on a device that doesn't move workspaces", () => {
		renderSection(DeviceMoves, MAC, "Mac");
		expect(screen.queryByRole("region", { name: /Moves into MaxHQ/ })).toBeNull();
	});
});
