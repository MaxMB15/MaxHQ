// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

const renderDevbox = () => {
	const source = mockSource({ now: () => NOW, tick: false, scenario: "connected" });
	const ui = createUiStore();
	ui.getState().setMode("devices");
	ui.getState().selectLink("link-devbox");
	render(
		<FixedNow value={NOW}>
			<App source={source} ui={ui} />
		</FixedNow>
	);
	return { source };
};

const card = async () =>
	within(
		within(await screen.findByRole("region", { name: "Workspaces on devbox" })).getByRole(
			"group",
			{ name: "Workspace mhq-maxhq" }
		)
	);

const openSnapshots = async () => {
	const workspace = await card();
	expect(workspace.getByText(/^3 changed files and 1 unpushed commit\./)).toBeInTheDocument();
	fireEvent.click(workspace.getByRole("button", { name: "Snapshots" }));
	return within(await workspace.findByRole("group", { name: "Snapshots of mhq-maxhq" }));
};

describe("a workspace's snapshots", () => {
	it("lists them and restores one as a new workspace", async () => {
		const { source } = renderDevbox();
		const list = await openSnapshots();
		expect(
			await list.findByRole("listitem", { name: "Snapshot snap-before-move" })
		).toHaveTextContent("kept until you delete it");
		const restore = vi.spyOn(source, "restoreSnapshot");
		fireEvent.click(
			list.getByRole("button", { name: "Restore snap-daily-1 as a new workspace" })
		);
		expect(restore).toHaveBeenCalledWith("ws-mhq-maxhq", "snap-daily-1", true, false);
		expect(await list.findByText("Made a new workspace from snap-daily-1")).toBeInTheDocument();
	});

	it("says what a roll back loses and asks before it throws that away", async () => {
		const { source } = renderDevbox();
		const list = await openSnapshots();
		const restore = vi.spyOn(source, "restoreSnapshot");
		fireEvent.click(
			await list.findByRole("button", { name: "Roll mhq-maxhq back to snap-daily-2" })
		);
		expect(restore).not.toHaveBeenCalled();
		const ask = list.getByRole("alert", { name: "Roll back to snap-daily-2" });
		expect(within(ask).getByText(/^Rolling back loses this: /)).toBeInTheDocument();

		fireEvent.click(within(ask).getByRole("button", { name: "Keep it" }));
		expect(list.queryByRole("alert", { name: "Roll back to snap-daily-2" })).toBeNull();

		fireEvent.click(list.getByRole("button", { name: "Roll mhq-maxhq back to snap-daily-2" }));
		fireEvent.click(
			within(list.getByRole("alert", { name: "Roll back to snap-daily-2" })).getByRole(
				"button",
				{ name: "Discard and roll back" }
			)
		);
		expect(restore).toHaveBeenCalledWith("ws-mhq-maxhq", "snap-daily-2", false, true);
		await waitFor(() =>
			expect(list.getByText("Rolled back to snap-daily-2")).toBeInTheDocument()
		);
	});

	it("shows the device's words when it refuses", async () => {
		const { source } = renderDevbox();
		vi.spyOn(source, "restoreSnapshot").mockResolvedValue({
			status: "error",
			error: { kind: "failed", message: "the pool is full" },
		});
		const list = await openSnapshots();
		fireEvent.click(
			await list.findByRole("button", { name: "Restore snap-daily-1 as a new workspace" })
		);
		expect(await list.findByText("The pool is full")).toBeInTheDocument();
	});
});
