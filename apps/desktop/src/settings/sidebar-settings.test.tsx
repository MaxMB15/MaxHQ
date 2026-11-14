// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import {
	act,
	fireEvent,
	type queries,
	render,
	screen,
	waitFor,
	within,
} from "@testing-library/react";
import { describe, expect, it } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import { PREFS_KEY } from "@/sidebar/prefs";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";
import { DEFAULT_SNOOZE_PRESETS, type SnoozePresetSetting } from "@maxhq/shared/view/snooze";

// NOW is a Sunday at 14:48, so a Monday option wakes when Tomorrow does.
const DEFAULT_MENU = "Menu now: In 1 hour · In 3 hours · This evening · Tomorrow";

const openSidebarSettings = async () => {
	render(
		<FixedNow value={NOW}>
			<App
				source={mockSource({ now: () => NOW, tick: false, scenario: "connected" })}
				ui={createUiStore()}
			/>
		</FixedNow>
	);
	await screen.findAllByRole("button", { name: /devbox/ });
	fireEvent.click(screen.getByRole("button", { name: "Settings" }));
	const nav = await screen.findByRole("navigation", { name: "Settings pages" });
	fireEvent.click(
		within(within(nav).getByRole("group", { name: "App" })).getByRole("button", {
			name: "Sidebar",
		})
	);
	const dialog = await screen.findByRole("dialog", { name: "Settings" });
	return within(within(dialog).getByRole("region", { name: "Snooze options" }));
};

const saved = (): { workingShelf: boolean; snoozePresets: SnoozePresetSetting[] } | null =>
	JSON.parse(localStorage.getItem(PREFS_KEY) ?? "null");

type Editor = ReturnType<typeof within<typeof queries>>;

const rows = (editor: Editor) =>
	editor.getAllByRole("group", { name: /^Snooze option/ }).map((row) => row.textContent);

const option = (editor: Editor, n: number) =>
	within(editor.getByRole("group", { name: `Snooze option ${n}` }));

const choose = async (box: HTMLElement, label: string) => {
	fireEvent.click(box);
	const found = (await screen.findAllByRole("option")).find((o) => o.textContent === label)!;
	fireEvent.pointerDown(found, { pointerType: "mouse" });
	fireEvent.click(found);
};

describe("the sidebar settings page", () => {
	it("keeps busy agents in their projects until you turn the Working shelf on", async () => {
		await openSidebarSettings();
		const shelf = screen.getByRole("switch", { name: "Working shelf" });
		expect(shelf).not.toBeChecked();
		fireEvent.click(shelf);
		expect(shelf).toBeChecked();
		expect(saved()?.workingShelf).toBe(true);
	});

	it("starts with the default options, soonest first", async () => {
		const editor = await openSidebarSettings();
		expect(rows(editor)).toEqual([
			"InhourIn 1 hour15:48",
			"InhoursIn 3 hours17:48",
			"TodayatThis evening18:00",
			"TomorrowatTomorrow09:00",
			"MondayatSame time as an option above",
		]);
		expect(option(editor, 1).getByRole("textbox", { name: "Amount" })).toHaveValue("1");
		expect(option(editor, 1).getByRole("combobox", { name: "Unit, hour" })).toBeInTheDocument();
		expect(option(editor, 4).getByLabelText("Time")).toHaveValue("09:00");
		expect(editor.getByText(DEFAULT_MENU)).toBeInTheDocument();
		expect(editor.queryByRole("button", { name: "Reset to defaults" })).toBeNull();
		expect(saved()).toBeNull();
	});
});

describe("the snooze options editor", () => {
	it("removes an option and puts the defaults back", async () => {
		const editor = await openSidebarSettings();
		fireEvent.click(editor.getByRole("button", { name: "Remove snooze option 1" }));
		expect(rows(editor)).toHaveLength(4);
		expect(saved()?.snoozePresets).toEqual(DEFAULT_SNOOZE_PRESETS.slice(1));
		expect(
			editor.getByText("Menu now: In 3 hours · This evening · Tomorrow")
		).toBeInTheDocument();

		fireEvent.click(editor.getByRole("button", { name: "Reset to defaults" }));
		expect(rows(editor)).toHaveLength(5);
		expect(editor.getByText(DEFAULT_MENU)).toBeInTheDocument();
		expect(saved()?.snoozePresets).toHaveLength(5);
		expect(editor.queryByRole("button", { name: "Reset to defaults" })).toBeNull();
	});

	it("says when nothing is left but Custom", async () => {
		const editor = await openSidebarSettings();
		for (let i = 0; i < 5; i++) {
			fireEvent.click(editor.getByRole("button", { name: "Remove snooze option 1" }));
		}
		expect(saved()?.snoozePresets).toEqual([]);
		expect(editor.getByText("Menus offer only Custom…")).toBeInTheDocument();
	});

	it("adds options up to eight", async () => {
		const editor = await openSidebarSettings();
		const add = editor.getByRole("button", { name: "Add option" });
		for (let i = 0; i < 3; i++) fireEvent.click(add);
		expect(add).toBeDisabled();
		expect(editor.getByText("Up to 8 options")).toBeInTheDocument();
		expect(saved()?.snoozePresets.slice(5)).toEqual([
			{ kind: "after", minutes: 30 },
			{ kind: "at", day: "saturday", time: "09:00" },
			{ kind: "after", minutes: 24 * 60 },
		]);
		// New rows wait at the bottom until focus leaves the list.
		expect(rows(editor).at(-1)).toContain("In 1 day");
	});

	it("sorts a new row into place once focus leaves the list", async () => {
		const editor = await openSidebarSettings();
		const add = editor.getByRole("button", { name: "Add option" });
		fireEvent.click(add);
		expect(rows(editor).at(-1)).toBe("InminutesIn 30 minutes15:18");
		fireEvent.focusIn(add);
		fireEvent.focusOut(add, {
			relatedTarget: editor.getByRole("button", { name: "Add option" }),
		});
		await Promise.resolve();
		expect(rows(editor).at(-1)).toBe("InminutesIn 30 minutes15:18");
		// A picker's options open outside the list, and moving into them isn't leaving it.
		const picker = document.createElement("div");
		picker.dataset.snoozePicker = "";
		const choice = picker.appendChild(document.createElement("button"));
		document.body.append(picker);
		fireEvent.focusOut(add, { relatedTarget: choice });
		picker.remove();
		await act(() => Promise.resolve());
		expect(rows(editor).at(-1)).toBe("InminutesIn 30 minutes15:18");
		// Focus that leaves for the page leaves the list, even with the sidebar's list on it.
		fireEvent.focusOut(add, { relatedTarget: document.body });
		await waitFor(() => expect(rows(editor)[0]).toBe("InminutesIn 30 minutes15:18"));
	});

	it("saves an amount once the field commits", async () => {
		const editor = await openSidebarSettings();
		const amount = option(editor, 1).getByRole("textbox", { name: "Amount" });
		fireEvent.change(amount, { target: { value: "2" } });
		expect(saved()).toBeNull();
		fireEvent.keyDown(amount, { key: "Enter" });
		expect(saved()?.snoozePresets[0]).toEqual({ kind: "after", minutes: 120 });
		expect(option(editor, 1).getByText("In 2 hours")).toBeInTheDocument();

		fireEvent.change(amount, { target: { value: "soon" } });
		fireEvent.blur(amount);
		expect(amount).toHaveValue("2");
		expect(saved()?.snoozePresets[0]).toEqual({ kind: "after", minutes: 120 });
	});

	it("keeps an amount within 30 days", async () => {
		const editor = await openSidebarSettings();
		const second = option(editor, 2);
		await choose(second.getByRole("combobox", { name: "Unit, hours" }), "days");
		expect(saved()?.snoozePresets).toContainEqual({ kind: "after", minutes: 3 * 24 * 60 });
		const amount = second.getByRole("textbox", { name: "Amount" });
		fireEvent.change(amount, { target: { value: "45" } });
		fireEvent.blur(amount);
		expect(amount).toHaveValue("30");
		expect(saved()?.snoozePresets).toContainEqual({ kind: "after", minutes: 30 * 24 * 60 });
		expect(second.getByRole("combobox", { name: "Unit, days" })).toBeInTheDocument();
	});

	it("switches an option to a day and back without losing its amount", async () => {
		const editor = await openSidebarSettings();
		const second = option(editor, 2);
		await choose(second.getByRole("combobox", { name: "When, In" }), "Saturday");
		expect(saved()?.snoozePresets).toContainEqual({
			kind: "at",
			day: "saturday",
			time: "09:00",
		});
		expect(saved()?.snoozePresets).not.toContainEqual({ kind: "after", minutes: 180 });
		expect(second.getByLabelText("Time")).toHaveValue("09:00");

		await choose(second.getByRole("combobox", { name: "When, Saturday" }), "In");
		expect(saved()?.snoozePresets).toContainEqual({ kind: "after", minutes: 180 });
		expect(second.getByRole("textbox", { name: "Amount" })).toHaveValue("3");
	});

	it("says why a row isn't in the menu", async () => {
		const editor = await openSidebarSettings();
		fireEvent.change(option(editor, 3).getByLabelText("Time"), { target: { value: "10:00" } });
		expect(option(editor, 3).getByText("Passed for today")).toBeInTheDocument();
		fireEvent.change(option(editor, 2).getByRole("textbox", { name: "Amount" }), {
			target: { value: "1" },
		});
		fireEvent.blur(option(editor, 2).getByRole("textbox", { name: "Amount" }));
		expect(option(editor, 2).getByText("Duplicate of an option above")).toBeInTheDocument();
		expect(editor.getByText("Menu now: In 1 hour · Tomorrow")).toBeInTheDocument();
	});

	it("takes up options another window saved", async () => {
		const editor = await openSidebarSettings();
		const newValue = JSON.stringify({
			version: 1,
			workingShelf: true,
			snoozePresets: [{ kind: "after", minutes: 30 }],
		});
		act(() => {
			window.dispatchEvent(new StorageEvent("storage", { key: PREFS_KEY, newValue }));
		});
		expect(rows(editor)).toEqual(["InminutesIn 30 minutes15:18"]);
		expect(screen.getByRole("switch", { name: "Working shelf" })).toBeChecked();
	});
});
