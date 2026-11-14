// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import type { Entity } from "@maxhq/shared/bindings";
import type { DataSource } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { MIN, NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

// The mock source, with this Mac's maxd stopped when `down`.
const sourceWith = (down: boolean): DataSource => {
	const mock = mockSource({ now: () => NOW, tick: false });
	if (!down) return mock;
	return {
		...mock,
		snapshot: async () => {
			const snapshot = await mock.snapshot();
			const entities = snapshot.entities.map((e): Entity =>
				e.entity === "device_link" && e.id === "link-mac"
					? {
							...e,
							state: {
								state: "unreachable",
								reason: "maxd isn't running",
								last_seen: NOW - 5 * MIN,
							},
						}
					: e
			);
			return { ...snapshot, entities };
		},
	};
};

const renderApp = ({ down, link }: { down: boolean; link?: string }) => {
	const source = sourceWith(down);
	const ui = createUiStore();
	if (link) {
		ui.getState().setMode("devices");
		ui.getState().selectLink(link);
	}
	render(
		<FixedNow value={NOW}>
			<App source={source} ui={ui} />
		</FixedNow>
	);
	return { source, ui };
};

const section = () => screen.findByRole("region", { name: "maxd on Mac" });

const failed = (message: string) =>
	({ status: "error", error: { kind: "failed", message } }) as const;

const MISPLACED =
	"Move MaxHQ to your Applications folder, open it from there, then start maxd again.";

describe("starting this Mac's maxd", () => {
	it("offers Start maxd on this Mac's page while its maxd isn't running", async () => {
		const { source } = renderApp({ down: true, link: "link-mac" });
		const start = vi.spyOn(source, "startLocalMaxd");
		const row = await section();
		fireEvent.click(within(row).getByRole("button", { name: "Start maxd" }));
		expect(start).toHaveBeenCalledTimes(1);
		// The header has one too, for a page that scrolls.
		expect(screen.getAllByRole("button", { name: "Start maxd" })).toHaveLength(2);
	});

	it("says why maxd didn't start", async () => {
		const { source } = renderApp({ down: true, link: "link-mac" });
		vi.spyOn(source, "startLocalMaxd").mockResolvedValue(failed(MISPLACED));
		const row = await section();
		fireEvent.click(within(row).getByRole("button", { name: "Start maxd" }));
		expect(await within(row).findByText(MISPLACED)).toBeInTheDocument();
		expect(within(row).getByRole("button", { name: "Start maxd" })).toBeEnabled();
	});

	it("has no Start maxd while this Mac's maxd is connected", async () => {
		renderApp({ down: false, link: "link-mac" });
		const row = await section();
		expect(within(row).getByRole("button", { name: "Uninstall maxd" })).toBeInTheDocument();
		expect(screen.queryByRole("button", { name: "Start maxd" })).toBeNull();
	});

	it("leaves maxd alone on another device's page", async () => {
		renderApp({ down: true, link: "link-devbox" });
		await screen.findByRole("region", { name: "Restart devbox" });
		expect(screen.queryByRole("region", { name: "maxd on devbox" })).toBeNull();
		expect(screen.queryByRole("button", { name: "Start maxd" })).toBeNull();
	});
});

describe("the notice while this Mac's maxd isn't running", () => {
	it("says so with no pane open and opens this Mac's page", async () => {
		const { ui } = renderApp({ down: true });
		const notice = await screen.findByRole("region", { name: "maxd on this Mac" });
		expect(notice).toHaveTextContent("Can't reach maxd on this Mac");
		expect(notice).toHaveTextContent("maxd isn't running. Start it to run agents on this Mac.");

		fireEvent.click(within(notice).getByRole("button", { name: "Open Mac" }));
		const state = ui.getState();
		expect(state.mode).toBe("devices");
		expect(state.selectedLink).toBe("link-mac");
		expect(await section()).toBeInTheDocument();
	});

	it("starts maxd and goes once MaxHQ is reaching it", async () => {
		const { source } = renderApp({ down: true });
		const start = vi.spyOn(source, "startLocalMaxd");
		const notice = await screen.findByRole("region", { name: "maxd on this Mac" });
		fireEvent.click(within(notice).getByRole("button", { name: "Start maxd" }));
		expect(start).toHaveBeenCalledTimes(1);
		await waitFor(() =>
			expect(screen.queryByRole("region", { name: "maxd on this Mac" })).toBeNull()
		);
	});

	it("isn't there while this Mac's maxd is connected", async () => {
		renderApp({ down: false });
		expect(await screen.findByText("Pick an agent to continue")).toBeInTheDocument();
		expect(screen.queryByRole("region", { name: "maxd on this Mac" })).toBeNull();
	});
});

describe("uninstalling this Mac's maxd", () => {
	const open = async () => {
		fireEvent.click(within(await section()).getByRole("button", { name: "Uninstall maxd" }));
		return within(await screen.findByRole("dialog", { name: "Uninstall maxd on this Mac?" }));
	};
	const WIPE = "Also delete MaxHQ's data on this Mac";

	it("says what it means and keeps the data unless you tick it", async () => {
		const { source } = renderApp({ down: false, link: "link-mac" });
		const uninstall = vi.spyOn(source, "uninstallLocalMaxd");
		const dialog = await open();
		for (const line of [
			"Every agent on this Mac stops.",
			"Your phone and your other devices can't reach this Mac.",
			"maxd no longer starts when you log in.",
			"That's your agent history, the notes kept on this Mac, your settings and your keys. MaxHQ quits once it's done.",
			"To remove MaxHQ, uninstall maxd here, then move MaxHQ to the Trash. If you trash MaxHQ first, maxd removes itself within about five minutes.",
		]) {
			expect(dialog.getByText(line)).toBeInTheDocument();
		}
		expect(dialog.getByRole("checkbox", { name: WIPE })).not.toBeChecked();

		fireEvent.click(dialog.getByRole("button", { name: "Uninstall maxd" }));
		expect(uninstall).toHaveBeenCalledWith(false);
		await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
		expect(
			await within(await section()).findByText(
				"maxd is uninstalled. Start maxd puts it back."
			)
		).toBeInTheDocument();
	});

	it("deletes MaxHQ's data too when you tick it, then says MaxHQ is quitting", async () => {
		const { source } = renderApp({ down: false, link: "link-mac" });
		const uninstall = vi.spyOn(source, "uninstallLocalMaxd");
		const dialog = await open();
		fireEvent.click(dialog.getByRole("checkbox", { name: WIPE }));
		fireEvent.click(dialog.getByRole("button", { name: "Uninstall and delete" }));
		expect(uninstall).toHaveBeenCalledWith(true);
		expect(await dialog.findByText("Done. MaxHQ is quitting.")).toBeInTheDocument();
		expect(dialog.getByRole("button", { name: "Uninstall and delete" })).toBeDisabled();
		expect(dialog.getByRole("button", { name: "Cancel" })).toBeDisabled();
	});

	it("says why it didn't uninstall and stays open", async () => {
		const { source } = renderApp({ down: false, link: "link-mac" });
		vi.spyOn(source, "uninstallLocalMaxd").mockResolvedValue(failed("An agent would not stop"));
		const dialog = await open();
		fireEvent.click(dialog.getByRole("button", { name: "Uninstall maxd" }));
		expect(await dialog.findByText("An agent would not stop")).toBeInTheDocument();
		expect(dialog.getByRole("button", { name: "Uninstall maxd" })).toBeEnabled();
	});

	it("does nothing when you cancel", async () => {
		const { source } = renderApp({ down: false, link: "link-mac" });
		const uninstall = vi.spyOn(source, "uninstallLocalMaxd");
		const dialog = await open();
		fireEvent.click(dialog.getByRole("button", { name: "Cancel" }));
		await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
		expect(uninstall).not.toHaveBeenCalled();
	});
});
