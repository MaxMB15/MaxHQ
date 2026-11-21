// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import App from "@/App";
import { setThemeChoice } from "@/app/theme";
import { mockSource } from "@/data/mock-source";
import { openSettingsWindow } from "@/test/settings";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { setLanguageChoice, configureLanguage } from "@maxhq/shared/i18n";
import { THEME_KEY } from "@maxhq/shared/theme/theme";
import { createUiStore } from "@maxhq/shared/ui/store";

import { AppearanceSettingsPage } from "./appearance-settings";

afterEach(() => {
	setThemeChoice("system");
	setLanguageChoice("en");
});

const choose = async (picker: HTMLElement, label: string) => {
	fireEvent.click(picker);
	const found = (await screen.findAllByRole("option")).find((o) => o.textContent === label)!;
	fireEvent.pointerDown(found, { pointerType: "mouse" });
	fireEvent.click(found);
};

describe("the appearance settings page", () => {
	it("sits in General, and the search finds it by its words", async () => {
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
		fireEvent.change(within(nav).getByRole("searchbox", { name: "Search settings" }), {
			target: { value: "dark" },
		});
		expect(within(nav).getByRole("button", { name: "General" })).toBeVisible();
		expect(within(nav).queryByRole("button", { name: "Notifications" })).toBeNull();
		fireEvent.click(within(nav).getByRole("button", { name: /^Appearance/ }));
		const card = within(within(dialog).getByRole("region", { name: "Appearance" }));
		expect(card.getByRole("combobox", { name: "Theme, System" })).toBeInTheDocument();
		expect(within(dialog).queryByRole("region", { name: "Updates" })).toBeNull();
	});

	it("starts on System, and shows and saves the theme you pick", async () => {
		render(<AppearanceSettingsPage />);
		const page = within(screen.getByRole("region", { name: "Appearance" }));
		await choose(page.getByRole("combobox", { name: "Theme, System" }), "Light");
		expect(document.documentElement.dataset.theme).toBe("light");
		expect(localStorage.getItem(THEME_KEY)).toBe("light");
		await choose(page.getByRole("combobox", { name: "Theme, Light" }), "Dark");
		expect(document.documentElement.dataset.theme).toBe("dark");
		expect(localStorage.getItem(THEME_KEY)).toBe("dark");
		expect(page.getByRole("combobox", { name: "Theme, Dark" })).toBeInTheDocument();
	});
});

it("changes the interface language immediately and follows System when selected", async () => {
	const dispose = configureLanguage({
		read: () => "en",
		write: () => {},
		systemLanguages: () => ["de-DE"],
	});
	render(<AppearanceSettingsPage />);
	await choose(screen.getByRole("combobox", { name: "Language, English" }), "Nederlands");
	expect(screen.getByRole("combobox", { name: "Taal, Nederlands" })).toBeInTheDocument();
	await choose(screen.getByRole("combobox", { name: "Taal, Nederlands" }), "Systeem");
	expect(screen.getByRole("combobox", { name: "Sprache, System" })).toBeInTheDocument();
	dispose();
});
