// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import { FOLDERS_SINCE, MOVE_AGENT_SINCE } from "@maxhq/shared/bindings";
import { DEVBOX, DEVBOX_CLAUDE, MAC, MHQ_WORKSPACE } from "@maxhq/shared/data/mock-view";
import type { DataSource } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

// The app on the sample source, with every device's maxd speaking `protocol`.
const renderApp = (protocol: number = MOVE_AGENT_SINCE) => {
	const source = mockSource({ now: () => NOW, tick: false });
	const snapshot = source.snapshot;
	const speaking: DataSource["snapshot"] = async () => {
		const state = await snapshot();
		return {
			...state,
			entities: state.entities.map((e) =>
				e.entity === "device_link" ? { ...e, protocol } : e
			),
		};
	};
	Object.assign(source, { snapshot: speaking });
	render(
		<FixedNow value={NOW}>
			<App source={source} ui={createUiStore()} />
		</FixedNow>
	);
	return source;
};

const openAgent = async (project: string, title: RegExp) => {
	const nav = screen.getByRole("navigation", { name: "Agents" });
	const region = await within(nav).findByRole("region", { name: project });
	fireEvent.click(within(region).getByRole("option", { name: title }));
	await screen.findByRole("heading", { level: 1, name: title });
};

const moveItem = async () => {
	fireEvent.click(screen.getByRole("button", { name: "More" }));
	return screen.findByRole("menuitem", { name: /Move to…/ });
};

const openMove = async (title: string) => {
	fireEvent.click(await moveItem());
	return within(await screen.findByRole("dialog", { name: `Move ${title} to another device` }));
};

/** Picks Another folder… in the move form, and gives its path field. */
const anotherFolder = async (dialog: ReturnType<typeof within>) => {
	fireEvent.click(dialog.getByRole("combobox", { name: /^Folder,/ }));
	const options = await screen.findAllByRole("option");
	const another = options.find((o) => o.textContent?.includes("Another folder…"));
	if (!another) throw new Error("No Another folder… option");
	fireEvent.pointerDown(another, { pointerType: "mouse" });
	fireEvent.click(another);
	return dialog.findByRole("textbox", { name: "Folder" });
};

describe("moving an agent", () => {
	it("offers the accounts there, and a way to sign in to a vendor that isn't", async () => {
		renderApp();
		await openAgent("Comet", /Seek bar thumbnails/);
		const dialog = await openMove("Seek bar thumbnails");
		expect(dialog.getByRole("combobox", { name: "Device, devbox" })).toBeInTheDocument();
		expect(dialog.getByRole("combobox", { name: /^Account, Claude/ })).toBeInTheDocument();
		// devbox checks out Comet too.
		expect(dialog.getByRole("combobox", { name: "Folder, Comet" })).toBeInTheDocument();
		expect(dialog.getByText("Not signed in on devbox")).toBeInTheDocument();
		expect(dialog.getByText("It goes on with Claude on devbox.")).toBeInTheDocument();
		// It isn't in a turn, so nothing stops first.
		expect(dialog.getByRole("button", { name: "Move" })).toBeEnabled();

		fireEvent.click(dialog.getByRole("button", { name: "Sign in to Cursor" }));
		const settings = await screen.findByRole("dialog", { name: "Settings" });
		expect(
			within(settings).getByRole("heading", { level: 2, name: "Cursor in workspaces" })
		).toBeInTheDocument();
	});

	it("stops a turn, moves it to the same repository there, and links both agents", async () => {
		const source = renderApp();
		const move = vi.spyOn(source, "moveAgent");
		await openAgent("MaxHQ", /Release checklist/);
		const dialog = await openMove("Release checklist");
		expect(dialog.getByRole("combobox", { name: "Folder, MaxHQ" })).toBeInTheDocument();
		expect(dialog.getByText("Its current turn stops.")).toBeInTheDocument();
		expect(dialog.getByText("2 automations will follow it.")).toBeInTheDocument();
		fireEvent.click(dialog.getByRole("button", { name: "Stop and move" }));

		await waitFor(() =>
			expect(move).toHaveBeenCalledWith(
				MAC,
				"agent-release",
				MAC,
				{
					device: DEVBOX,
					account: DEVBOX_CLAUDE,
					model: null,
					place: { type: "workspace", workspace: MHQ_WORKSPACE },
				},
				true
			)
		);
		// The new agent opens, and leads back to the old one.
		const from = await screen.findByRole("note", { name: "Moved from Mac" });
		expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
		fireEvent.click(within(from).getByRole("button", { name: "Open the old agent" }));
		const banner = await screen.findByRole("region", { name: "Moved to devbox" });
		expect(screen.queryByRole("button", { name: "Resume" })).not.toBeInTheDocument();
		fireEvent.click(within(banner).getByRole("button", { name: "Open" }));
		expect(await screen.findByRole("note", { name: "Moved from Mac" })).toBeInTheDocument();
	});

	it("suggests folders on the device it moves to, and browses them", async () => {
		renderApp(FOLDERS_SINCE);
		await openAgent("Comet", /Seek bar thumbnails/);
		const dialog = await openMove("Seek bar thumbnails");
		const field = await anotherFolder(dialog);
		act(() => field.focus());
		fireEvent.change(field, { target: { value: "~/code/m" } });
		const list = await dialog.findByRole("listbox", { name: "Suggestions" });
		expect(within(list).getByRole("option")).toHaveTextContent("~/code/Mosaicgit · main");
		fireEvent.click(dialog.getByRole("button", { name: "Browse…" }));
		expect(
			await screen.findByRole("dialog", { name: "Choose a folder on devbox" })
		).toBeInTheDocument();
	});

	it("can't browse a device whose maxd doesn't list folders", async () => {
		renderApp(FOLDERS_SINCE - 1);
		await openAgent("Comet", /Seek bar thumbnails/);
		const dialog = await openMove("Seek bar thumbnails");
		await anotherFolder(dialog);
		const browse = dialog.getByRole("button", { name: "Browse…" });
		expect(browse).toHaveAttribute("aria-disabled", "true");
		fireEvent.click(browse);
		expect(screen.queryByRole("dialog", { name: /Choose a folder/ })).toBeNull();
	});

	it("says a maxd too old to move it needs updating", async () => {
		renderApp(MOVE_AGENT_SINCE - 1);
		await openAgent("MaxHQ", /Release checklist/);
		const item = await moveItem();
		expect(item).toHaveAttribute("aria-disabled", "true");
		expect(item).toHaveTextContent("Moving needs a newer maxd on Mac.");
	});
});
