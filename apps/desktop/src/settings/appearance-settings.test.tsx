// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import App from "@/App";
import { setThemeChoice } from "@/app/theme";
import { mockSource } from "@/data/mock-source";
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
	it("sits in the App group, and the search finds it by its words", async () => {
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
		fireEvent.change(within(nav).getByRole("searchbox", { name: "Search settings" }), {
			target: { value: "dark" },
		});
		const app = within(nav).getByRole("group", { name: "App" });
		const [page, ...rest] = within(app).getAllByRole("button");
		expect(rest).toEqual([]);
		expect(page).toHaveTextContent(/^Appearance/);
		fireEvent.click(page!);
		const dialog = screen.getByRole("dialog", { name: "Settings" });
		const colors = within(within(dialog).getByRole("region", { name: "Colors" }));
		expect(colors.getByRole("combobox", { name: "Theme, System" })).toBeInTheDocument();
	});

	it("starts on System, and shows and saves the theme you pick", async () => {
		render(<AppearanceSettingsPage />);
		const page = within(screen.getByRole("region", { name: "Colors" }));
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
