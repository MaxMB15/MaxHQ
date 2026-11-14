// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The look this Mac's windows take. The choice lives in localStorage, where
// the boot script (public/theme-boot.js) reads it before the first paint,
// and every window takes up a choice another saves. System follows the
// computer's appearance as it changes. In the app, the window's own parts,
// its traffic lights, menus and sheets, follow the choice too.

import { getCurrentWindow } from "@tauri-apps/api/window";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

import { inApp } from "@/native/platform";
import {
	type Theme,
	THEME_KEY,
	type ThemeChoice,
	themeChoiceOf,
	themeFor,
} from "@maxhq/shared/theme/theme";

const DARK_QUERY = "(prefers-color-scheme: dark)";

/** What was picked, and the look it gives now. */
type Look = { readonly choice: ThemeChoice; readonly theme: Theme };

/**
 * Whether the computer is in dark mode. A page that can't tell, such as a
 * test's, reads as dark, the look the app had before it had a light one.
 */
const systemDark = (): boolean =>
	typeof matchMedia !== "function" || matchMedia(DARK_QUERY).matches;

const lookOf = (choice: ThemeChoice): Look => ({ choice, theme: themeFor(choice, systemDark()) });

const loadChoice = (): ThemeChoice => {
	try {
		return themeChoiceOf(localStorage.getItem(THEME_KEY));
	} catch {
		return "system";
	}
};

const store = createStore<Look>(() => lookOf(loadChoice()));

/** Puts `theme` on the page, with every color changing in the same frame. */
const paint = (theme: Theme): void => {
	const root = document.documentElement;
	if (root.dataset.theme === theme) return;
	root.classList.add("theme-switching");
	root.dataset.theme = theme;
	root.classList.toggle("dark", theme === "dark");
	// Styles the page in the new colors before transitions come back.
	void root.offsetHeight;
	requestAnimationFrame(() => root.classList.remove("theme-switching"));
};

/** Has the window's own parts follow the choice, with System leaving them to the computer. */
const tellWindow = (choice: ThemeChoice): void => {
	if (!inApp()) return;
	getCurrentWindow()
		.setTheme(choice === "system" ? null : choice)
		.catch((error: unknown) => console.warn("Couldn't set the window's theme", error));
};

const show = (choice: ThemeChoice): void => {
	const before = store.getState();
	const look = lookOf(choice);
	if (look.choice !== before.choice || look.theme !== before.theme) store.setState(look);
	paint(look.theme);
	if (look.choice !== before.choice) tellWindow(choice);
};

/** Changes the theme and saves the choice. */
export const setThemeChoice = (choice: ThemeChoice): void => {
	show(choice);
	try {
		localStorage.setItem(THEME_KEY, choice);
	} catch (error) {
		console.warn("Couldn't save the theme", error);
	}
};

/**
 * Shows the saved theme and keeps it up: System follows the computer's
 * appearance, and a choice another window saves shows here too. Returns a
 * function that stops.
 */
export const watchTheme = (): (() => void) => {
	const choice = loadChoice();
	show(choice);
	tellWindow(choice);
	const query = typeof matchMedia === "function" ? matchMedia(DARK_QUERY) : null;
	const onSystem = () => show(store.getState().choice);
	const onStorage = (e: StorageEvent) => {
		if (e.key === THEME_KEY) show(themeChoiceOf(e.newValue));
	};
	query?.addEventListener("change", onSystem);
	window.addEventListener("storage", onStorage);
	return () => {
		query?.removeEventListener("change", onSystem);
		window.removeEventListener("storage", onStorage);
	};
};

/** The theme picked in Settings. */
export const useThemeChoice = (): ThemeChoice => useStore(store, (s) => s.choice);

/** The look the window shows now. */
export const useTheme = (): Theme => useStore(store, (s) => s.theme);

/** The look the window shows now, outside React. */
export const currentTheme = (): Theme => store.getState().theme;
