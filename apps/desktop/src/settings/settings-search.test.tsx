// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import { openSettingsWindow } from "@/test/settings";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

afterEach(() => {
	vi.restoreAllMocks();
});

const openSettings = async () => {
	render(
		<FixedNow value={NOW}>
			<App
				source={mockSource({ now: () => NOW, tick: false, scenario: "connected" })}
				ui={createUiStore()}
			/>
		</FixedNow>
	);
	await screen.findAllByRole("button", { name: /devbox/ });
	const { dialog, nav } = await openSettingsWindow();
	const field = within(nav).getByRole("searchbox", { name: "Search settings" });
	return { nav, dialog, field };
};

const search = (field: HTMLElement, text: string) =>
	fireEvent.change(field, { target: { value: text } });

const marks = (element: HTMLElement) =>
	Array.from(element.querySelectorAll("mark"), (mark) => mark.textContent);

// The left column's group buttons, outside the views of their own.
const groupButtons = (nav: HTMLElement) =>
	within(nav)
		.getAllByRole("button")
		.filter((button) => button.closest("[role=group]") === null);

describe("the settings page", () => {
	it("lists the seven groups, then the views of their own, and opens on General", async () => {
		const { nav, dialog } = await openSettings();
		expect(groupButtons(nav).map((button) => button.textContent)).toEqual([
			"General",
			"Chat and agents",
			"Notifications",
			"Accounts",
			"Devices and phones",
			"Permissions",
			"Projects",
		]);
		const views = within(nav).getByRole("group", { name: "Their own views" });
		expect(
			within(views)
				.getAllByRole("button")
				.map((button) => button.querySelector("span")?.textContent)
		).toEqual(["Skills", "Scheduled tasks", "Local models", "Keyboard shortcuts"]);
		expect(within(nav).getByRole("button", { name: "General" })).toHaveAttribute(
			"aria-current",
			"location"
		);
		expect(within(dialog).getByRole("heading", { level: 2, name: "General" })).toBeVisible();
		expect(within(dialog).getByRole("region", { name: "Appearance" })).toBeInTheDocument();
	});

	it("let values select, but never a heading, a label, a button or a hidden secret", async () => {
		const { nav, dialog } = await openSettings();
		const chrome = "h1, h2, h3, label, legend, button, summary, [role=option], [role=switch]";
		const sweep = (name: string) => {
			// One class marks a value, so selection looks the same everywhere.
			const other = dialog.querySelectorAll(".select-text, .select-all");
			expect(
				Array.from(other, (element) => element.textContent),
				name
			).toEqual([]);
			const values = Array.from(dialog.querySelectorAll(".selectable"));
			const inChrome = values.filter((value) => value.closest(chrome) !== null);
			expect(
				inChrome.map((value) => value.textContent),
				name
			).toEqual([]);
			// Dots stand in for a secret, and stay out of a selection.
			const masked = Array.from(dialog.querySelectorAll("*")).filter(
				(element) =>
					element.children.length === 0 &&
					element.textContent?.includes("•") &&
					element.closest(".selectable, .select-none")?.classList.contains("selectable")
			);
			expect(
				masked.map((element) => element.textContent),
				name
			).toEqual([]);
		};
		const back = async () => {
			fireEvent.click(within(dialog).getByRole("button", { name: "Back" }));
			await within(dialog).findByRole("region", { name: "Logins" });
		};

		// The page, with each device's tab and the Phones tab.
		sweep("page");
		const tabs = within(dialog).getByRole("tablist", { name: "Device" });
		for (const tab of within(tabs).getAllByRole("tab")) {
			fireEvent.click(tab);
			sweep(tab.textContent ?? "");
		}

		// Each login.
		const logins = within(dialog).getByRole("region", { name: "Logins" });
		const rows = within(logins).getAllByRole("rowheader").length;
		for (let at = 0; at < rows; at++) {
			const row = within(within(dialog).getByRole("region", { name: "Logins" }))
				.getAllByRole("rowheader")
				[at]!.closest("tr")!;
			const name = row.querySelector("th")?.textContent ?? "";
			fireEvent.click(within(row).getAllByRole("button")[0]!);
			await within(dialog).findByRole("button", { name: "Back" });
			sweep(name);
			await back();
		}

		// Each project.
		const projects = within(dialog)
			.getAllByRole("button", { name: /^Open / })
			.map((button) => button.getAttribute("aria-label")!);
		expect(projects.length).toBeGreaterThan(0);
		for (const project of projects) {
			fireEvent.click(within(dialog).getByRole("button", { name: project }));
			await within(dialog).findByRole("button", { name: "Back" });
			sweep(project);
			for (const tab of within(dialog).queryAllByRole("tab")) {
				fireEvent.click(tab);
				sweep(`${project}, ${tab.textContent}`);
			}
			await back();
		}

		// The views of their own, but the keyboard shortcuts, which show over any view.
		const views = within(nav).getByRole("group", { name: "Their own views" });
		for (const name of ["Skills", "Scheduled tasks", "Local models"]) {
			fireEvent.click(within(views).getByRole("button", { name: new RegExp(`^${name}`) }));
			await within(dialog).findByRole("button", { name: "Back" });
			sweep(name);
			await back();
		}
	});
});

describe("the settings search", () => {
	it("keeps the cards that have the words, and marks them", async () => {
		const { nav, dialog, field } = await openSettings();
		search(field, "split");
		expect(groupButtons(nav)).toEqual([]);
		const views = within(nav).getByRole("group", { name: "Their own views" });
		const [hit, ...rest] = within(views).getAllByRole("button");
		expect(rest).toEqual([]);
		expect(hit).toHaveTextContent(/^Keyboard shortcutsSplit down$/);
		expect(marks(hit!)).toEqual(["Split"]);
		// The page leads there too.
		expect(
			within(dialog).getByRole("heading", { level: 2, name: "Their own views" })
		).toBeVisible();
		expect(within(dialog).queryByRole("region", { name: "Appearance" })).toBeNull();
	});

	it("matches every word, across the title and a label, and opens the first hit on Enter", async () => {
		const { nav, dialog, field } = await openSettings();
		search(field, "phones pairing");
		expect(groupButtons(nav).map((button) => button.textContent)).toEqual([
			"Devices and phones",
			"PhonesPairing code",
		]);
		const hit = within(nav).getByRole("button", { name: /^Phones/ });
		expect(marks(hit)).toEqual(["Phones", "Pairing"]);

		fireEvent.keyDown(field, { key: "Enter" });
		const tabs = within(dialog).getByRole("tablist", { name: "Device" });
		expect(within(tabs).getByRole("tab", { name: "Phones" })).toHaveAttribute(
			"aria-selected",
			"true"
		);
		expect(within(dialog).getByRole("region", { name: "Add phone" })).toBeInTheDocument();
	});

	it("finds the dispatcher's pause before a new chat", async () => {
		const { nav, dialog, field } = await openSettings();
		search(field, "chat pause");
		const hit = within(nav).getByRole("button", { name: /^Dispatcher/ });
		expect(hit).toHaveTextContent("DispatcherNew chat after a pause");
		expect(groupButtons(nav).map((button) => button.textContent)).toEqual([
			"Chat and agents",
			"DispatcherNew chat after a pause",
		]);

		fireEvent.click(hit);
		expect(
			within(dialog).getByRole("combobox", { name: /^New chat after a pause/ })
		).toBeInTheDocument();
		expect(within(dialog).queryByRole("region", { name: "Composer" })).toBeNull();
	});

	it("opens a project on its Skills tab when only that tab has the words", async () => {
		const { nav, dialog, field } = await openSettings();
		search(field, "proposals");
		const hit = within(nav).getByRole("button", { name: /^MaxHQ/ });
		expect(hit).toHaveTextContent("MaxHQProposals");

		fireEvent.click(hit);
		expect(await within(dialog).findByRole("tab", { name: "Skills" })).toHaveAttribute(
			"aria-selected",
			"true"
		);
		expect(within(dialog).getByRole("button", { name: "Back" })).toBeInTheDocument();
	});

	it("says when nothing matches, and Esc clears the search before it closes the settings", async () => {
		const { nav, field } = await openSettings();
		search(field, "nothing has this");
		expect(within(nav).getByRole("status")).toHaveTextContent("No settings match");
		expect(within(nav).queryAllByRole("button")).toEqual([]);

		fireEvent.keyDown(field, { key: "Escape" });
		expect(field).toHaveValue("");
		expect(within(nav).getByRole("button", { name: "Accounts" })).toBeInTheDocument();
		expect(screen.getByRole("dialog", { name: "Settings" })).toBeInTheDocument();

		fireEvent.keyDown(field, { key: "Escape" });
		await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
	});

	it("finds a device's own settings, and shows its tab at the card", async () => {
		const { nav, dialog, field } = await openSettings();
		search(field, "stop idle");
		const hits = groupButtons(nav).map((button) => button.textContent);
		expect(hits).toEqual([
			"Devices and phones",
			"MacStop idle CLIs after",
			"devboxStop idle CLIs after",
		]);
		const hit = within(nav).getByRole("button", { name: /^devbox/ });
		expect(marks(hit)).toEqual(["Stop", "idle"]);

		fireEvent.click(hit);
		const tabs = within(dialog).getByRole("tablist", { name: "Device" });
		expect(within(tabs).getByRole("tab", { name: "devbox" })).toHaveAttribute(
			"aria-selected",
			"true"
		);
		const card = within(dialog).getByRole("region", { name: "Resources" });
		expect(card.id).toBe("device-card-resources");
		expect(within(card).getByRole("region", { name: "Idle agents on devbox" })).toBeVisible();
		expect(screen.getByRole("dialog", { name: "Settings" })).toBeInTheDocument();
	});

	it("finds OpenRouter in Accounts, and opens its page", async () => {
		const { nav, dialog, field } = await openSettings();
		search(field, "openrouter");
		const hit = within(nav).getByRole("button", { name: /^OpenRouter/ });
		expect(marks(hit)).toContain("OpenRouter");
		expect(within(nav).getByRole("button", { name: /^Logins/ })).toBeInTheDocument();

		fireEvent.click(hit);
		expect(
			await within(dialog).findByRole("region", { name: "OpenRouter on Mac" })
		).toBeInTheDocument();
		expect(within(dialog).getByRole("button", { name: "Back" })).toBeInTheDocument();
	});

	it("takes Command-F to the field while the settings are open", async () => {
		vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
			"Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)"
		);
		const { nav, field } = await openSettings();
		const group = within(nav).getByRole("button", { name: "General" });
		act(() => group.focus());
		let passed = true;
		act(() => {
			passed = fireEvent.keyDown(group, { key: "f", code: "KeyF", metaKey: true });
		});
		expect(passed).toBe(false);
		expect(field).toHaveFocus();
	});
});
