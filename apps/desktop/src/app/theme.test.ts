// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { THEME_KEY, themeChoiceOf, themeFor } from "@maxhq/shared/theme/theme";

import boot from "../../public/theme-boot.js?raw";
import { currentTheme, setThemeChoice, useTheme, useThemeChoice, watchTheme } from "./theme";

const app = vi.hoisted(() => ({ inApp: false, setTheme: vi.fn(async () => {}) }));

vi.mock("@/native/platform", () => ({ inApp: () => app.inApp }));
vi.mock("@tauri-apps/api/window", () => ({
	getCurrentWindow: () => ({ setTheme: app.setTheme }),
}));

/** A computer whose appearance a test sets, and that says when it changes. */
const appearance = (dark: boolean) => {
	const listeners = new Set<() => void>();
	const query = {
		get matches() {
			return dark;
		},
		addEventListener: (_: string, listener: () => void) => listeners.add(listener),
		removeEventListener: (_: string, listener: () => void) => listeners.delete(listener),
	};
	vi.stubGlobal("matchMedia", () => query);
	return {
		set: (next: boolean) => {
			dark = next;
			for (const listener of listeners) listener();
		},
	};
};

const root = document.documentElement;

beforeEach(() => {
	app.inApp = false;
	app.setTheme.mockClear();
});

afterEach(() => {
	vi.unstubAllGlobals();
	setThemeChoice("system");
	delete root.dataset.theme;
	root.classList.remove("dark");
});

describe("the theme", () => {
	it("puts a choice on the page and saves it", () => {
		const choice = renderHook(() => useThemeChoice());
		const theme = renderHook(() => useTheme());
		act(() => setThemeChoice("light"));
		expect(root.dataset.theme).toBe("light");
		expect(root.classList.contains("dark")).toBe(false);
		expect(localStorage.getItem(THEME_KEY)).toBe("light");
		expect(choice.result.current).toBe("light");
		expect(theme.result.current).toBe("light");
		act(() => setThemeChoice("dark"));
		expect(root.dataset.theme).toBe("dark");
		expect(root.classList.contains("dark")).toBe(true);
		expect(currentTheme()).toBe("dark");
	});

	it("reads as dark on a page that can't tell the computer's appearance", () => {
		setThemeChoice("system");
		expect(currentTheme()).toBe("dark");
	});

	it("follows the computer's appearance on System, and only then", () => {
		const computer = appearance(true);
		const stop = watchTheme();
		expect(root.dataset.theme).toBe("dark");
		computer.set(false);
		expect(root.dataset.theme).toBe("light");
		setThemeChoice("dark");
		computer.set(true);
		computer.set(false);
		expect(root.dataset.theme).toBe("dark");
		stop();
		setThemeChoice("system");
		computer.set(true);
		expect(root.dataset.theme).toBe("light");
	});

	it("shows a choice another window saves", () => {
		appearance(true);
		const stop = watchTheme();
		window.dispatchEvent(new StorageEvent("storage", { key: THEME_KEY, newValue: "light" }));
		expect(root.dataset.theme).toBe("light");
		window.dispatchEvent(
			new StorageEvent("storage", { key: "maxhq.sidebar", newValue: "dark" })
		);
		expect(root.dataset.theme).toBe("light");
		// Cleared storage reads as System.
		window.dispatchEvent(new StorageEvent("storage", { key: THEME_KEY, newValue: null }));
		expect(root.dataset.theme).toBe("dark");
		stop();
	});

	it("has the app's window follow, leaving System to the computer", () => {
		app.inApp = true;
		localStorage.setItem(THEME_KEY, "light");
		const stop = watchTheme();
		expect(app.setTheme).toHaveBeenLastCalledWith("light");
		setThemeChoice("system");
		expect(app.setTheme).toHaveBeenLastCalledWith(null);
		setThemeChoice("dark");
		expect(app.setTheme).toHaveBeenLastCalledWith("dark");
		const calls = app.setTheme.mock.calls.length;
		setThemeChoice("dark");
		expect(app.setTheme).toHaveBeenCalledTimes(calls);
		stop();
	});

	it("leaves the window alone in a browser", () => {
		watchTheme()();
		setThemeChoice("light");
		expect(app.setTheme).not.toHaveBeenCalled();
	});
});

describe("the boot script", () => {
	it.each([null, "system", "light", "dark", "blue"])(
		"agrees with the app on a stored %s",
		(stored) => {
			for (const dark of [true, false]) {
				appearance(dark);
				if (stored === null) localStorage.removeItem(THEME_KEY);
				else localStorage.setItem(THEME_KEY, stored);
				new Function(boot)();
				const theme = themeFor(themeChoiceOf(stored), dark);
				expect(root.dataset.theme).toBe(theme);
				expect(root.classList.contains("dark")).toBe(theme === "dark");
			}
		}
	);

	it("reads storage that throws as System", () => {
		appearance(false);
		vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
			throw new Error("no storage");
		});
		new Function(boot)();
		expect(root.dataset.theme).toBe("light");
		vi.restoreAllMocks();
	});
});
