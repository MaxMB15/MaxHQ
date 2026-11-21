// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Opens Settings the way a person does, for tests that render the whole
// app: the header's gear, then a card on the page, a device's tab, a
// login's Manage, a project's Open or a view of its own from the left
// column.

import { fireEvent, screen, within } from "@testing-library/react";

/** Clicks the header's gear, unless the window is open already, and waits for it. */
export const openSettingsWindow = async (): Promise<{
	readonly dialog: HTMLElement;
	readonly nav: HTMLElement;
}> => {
	const open = screen.queryByRole("dialog", { name: "Settings" });
	if (!open) fireEvent.click(screen.getByRole("button", { name: "Settings" }));
	const dialog = open ?? (await screen.findByRole("dialog", { name: "Settings" }));
	return { dialog, nav: within(dialog).getByRole("navigation", { name: "Settings" }) };
};

/**
 * A card on the page, by its title. A group can share its card's name,
 * as Notifications does, and the card is the one inside.
 */
export const settingsCard = async (name: string | RegExp): Promise<HTMLElement> => {
	const { dialog } = await openSettingsWindow();
	return within(dialog).getAllByRole("region", { name }).at(-1)!;
};

/** Picks a tab of the Devices and phones group, by the device's name or Phones. */
export const openDeviceTab = async (name: string): Promise<HTMLElement> => {
	const { dialog } = await openSettingsWindow();
	const tabs = within(dialog).getByRole("tablist", { name: "Device" });
	fireEvent.click(within(tabs).getByRole("tab", { name }));
	return dialog;
};

/** Opens a view of its own from the left column, by its title. */
export const openOwnView = async (name: string): Promise<HTMLElement> => {
	const { dialog, nav } = await openSettingsWindow();
	const views = within(nav).getByRole("group", { name: "Their own views" });
	fireEvent.click(within(views).getByRole("button", { name: new RegExp(`^${name}`) }));
	await within(dialog).findByRole("button", { name: "Back" });
	return dialog;
};

/**
 * Opens a login's view from the accounts table, by the row's name, with
 * the first button that row shows: Manage, or Sign in on a device.
 */
export const openLogin = async (
	name: string,
	button: string | RegExp = /^(Manage|Sign in on)/
): Promise<HTMLElement> => {
	const { dialog } = await openSettingsWindow();
	const table = within(dialog).getByRole("region", { name: "Logins" });
	const row = within(table)
		.getByRole("rowheader", { name: new RegExp(`^${name}`) })
		.closest("tr")!;
	fireEvent.click(within(row).getAllByRole("button", { name: button })[0]!);
	await within(dialog).findByRole("button", { name: "Back" });
	return dialog;
};

/** Opens a project's settings from the Projects card, by its name. */
export const openProjectSettings = async (name: string): Promise<HTMLElement> => {
	const { dialog } = await openSettingsWindow();
	// The list fills once the view has the project.
	fireEvent.click(await within(dialog).findByRole("button", { name: `Open ${name}` }));
	await within(dialog).findByRole("button", { name: "Back" });
	return dialog;
};
