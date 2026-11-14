// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import type { DataSource } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

const renderApp = () => {
	const source: DataSource = mockSource({ now: () => NOW, tick: false });
	const ui = createUiStore();
	const deleteAgent = vi.spyOn(source, "deleteAgent");
	render(
		<FixedNow value={NOW}>
			<App source={source} ui={ui} />
		</FixedNow>
	);
	return { ui, deleteAgent };
};

const nav = () => screen.getByRole("navigation", { name: "Agents" });
// An agent's row in the sidebar's list, not the Needs you box, which has no menu and doesn't pick.
const row = (name: RegExp) => within(nav()).findByRole("option", { name });
const gone = (name: RegExp) =>
	waitFor(() => expect(within(nav()).queryByRole("option", { name })).toBeNull());

const menuFor = async (name: RegExp) => {
	fireEvent.contextMenu(await row(name));
	return screen.findByRole("menu");
};

const confirmation = () => screen.findByRole("alertdialog");

afterEach(() => {
	vi.restoreAllMocks();
});

describe("deleting an agent", () => {
	it("asks first, naming the agent, and does nothing when you cancel", async () => {
		const { deleteAgent } = renderApp();
		let menu = await menuFor(/Seek bar thumbnails/);
		fireEvent.click(within(menu).getByRole("menuitem", { name: "Delete" }));
		let dialog = await confirmation();
		expect(dialog).toHaveTextContent('Delete agent "Seek bar thumbnails"?');
		fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
		await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
		expect(deleteAgent).not.toHaveBeenCalled();

		menu = await menuFor(/Seek bar thumbnails/);
		fireEvent.click(within(menu).getByRole("menuitem", { name: "Delete" }));
		dialog = await confirmation();
		fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
		await gone(/Seek bar thumbnails/);
		expect(deleteAgent).toHaveBeenCalledWith("agent-cursor-seek");
	});

	it("isn't offered for an agent you started in a terminal", async () => {
		renderApp();
		const menu = await menuFor(/Bindings check/);
		expect(within(menu).getByRole("menuitem", { name: /^Settle/ })).toBeInTheDocument();
		expect(within(menu).queryByRole("menuitem", { name: /^Delete/ })).toBeNull();
	});

	it("deletes the picked agents MaxHQ holds, one after another, and ends the selection", async () => {
		const { ui, deleteAgent } = renderApp();
		fireEvent.click(await row(/Release checklist/), { ctrlKey: true });
		fireEvent.click(await row(/Seek bar thumbnails/), { ctrlKey: true });
		fireEvent.click(await row(/Bindings check/), { ctrlKey: true });

		fireEvent.click(screen.getByRole("button", { name: "Actions" }));
		const menu = await screen.findByRole("menu");
		fireEvent.click(within(menu).getByRole("menuitem", { name: "Delete (2)" }));
		const dialog = await confirmation();
		expect(dialog).toHaveTextContent("Delete 2 agents?");
		fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));

		await gone(/Seek bar thumbnails/);
		await gone(/Release checklist/);
		expect(deleteAgent.mock.calls).toEqual([["agent-release"], ["agent-cursor-seek"]]);
		// The one from a terminal stays picked.
		expect([...ui.getState().picked]).toEqual(["agent-bindings"]);
		expect(await row(/Bindings check/)).toBeInTheDocument();
	});

	it("says why a delete failed", async () => {
		const { deleteAgent } = renderApp();
		deleteAgent.mockResolvedValue({
			status: "ok",
			data: { status: "refused", reason: "the device is busy" },
		});
		const menu = await menuFor(/Seek bar thumbnails/);
		fireEvent.click(within(menu).getByRole("menuitem", { name: "Delete" }));
		fireEvent.click(within(await confirmation()).getByRole("button", { name: "Delete" }));

		expect(await within(nav()).findByRole("alert")).toHaveTextContent(
			"Couldn't delete Seek bar thumbnails. The device is busy."
		);
		expect(await row(/Seek bar thumbnails/)).toBeInTheDocument();
	});

	it("closes the pane of one beside the agent you're looking at", async () => {
		const { ui } = renderApp();
		fireEvent.click(await row(/Seek bar thumbnails/));
		await screen.findByRole("heading", { level: 1, name: /Seek bar thumbnails/ });
		fireEvent.click(await row(/Write the release notes/), { altKey: true });
		await screen.findByRole("heading", { level: 1, name: /Write the release notes/ });

		const menu = await menuFor(/Seek bar thumbnails/);
		fireEvent.click(within(menu).getByRole("menuitem", { name: "Delete" }));
		fireEvent.click(within(await confirmation()).getByRole("button", { name: "Delete" }));

		await gone(/Seek bar thumbnails/);
		expect(screen.queryByText(/This agent isn't here/)).toBeNull();
		expect(ui.getState().selectedAgent).toBe("agent-release-notes");
	});

	it("opens another agent in its project from the header's menu", async () => {
		const { ui, deleteAgent } = renderApp();
		const maxhq = within(await within(nav()).findByRole("region", { name: "MaxHQ" }));
		fireEvent.click(maxhq.getByRole("option", { name: /Write the release notes/ }));
		await screen.findByRole("heading", { level: 1, name: /Write the release notes/ });

		fireEvent.click(screen.getByRole("button", { name: "More" }));
		fireEvent.click(await screen.findByRole("menuitem", { name: /^Delete agent/ }));
		const dialog = await confirmation();
		expect(dialog).toHaveTextContent('Delete agent "Write the release notes"?');
		fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));

		await gone(/Write the release notes/);
		expect(deleteAgent).toHaveBeenCalledWith("agent-release-notes");
		await waitFor(() =>
			expect(screen.queryByRole("heading", { name: /Write the release notes/ })).toBeNull()
		);
		const next = ui.getState().selectedAgent;
		expect(next).not.toBeNull();
		expect(next).not.toBe("agent-release-notes");
		expect(await screen.findByRole("heading", { level: 1 })).toBeInTheDocument();
		// Its pane closed rather than saying the agent isn't here.
		expect(screen.queryByText(/This agent isn't here/)).toBeNull();
	});
});
