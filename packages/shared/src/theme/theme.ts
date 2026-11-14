// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Which look the app takes. The choice is System, Light or Dark, and System
// follows the computer's appearance as it changes. Anything stored that
// isn't one of the three reads as System, the default.

/** What the user picked in Settings. */
export type ThemeChoice = "system" | "light" | "dark";

/** The look the window shows. */
export type Theme = "light" | "dark";

export const THEME_CHOICES: readonly ThemeChoice[] = ["system", "light", "dark"];

/** Where the choice is kept, as the boot script reads it before the first paint. */
export const THEME_KEY = "maxhq.theme";

/** The stored choice, or System for anything else. */
export const themeChoiceOf = (stored: unknown): ThemeChoice =>
	THEME_CHOICES.includes(stored as ThemeChoice) ? (stored as ThemeChoice) : "system";

/** The look a choice gives, with the computer in dark mode or not. */
export const themeFor = (choice: ThemeChoice, systemDark: boolean): Theme =>
	choice === "system" ? (systemDark ? "dark" : "light") : choice;
