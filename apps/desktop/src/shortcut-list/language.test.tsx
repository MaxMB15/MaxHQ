// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";

import { setLanguageChoice } from "@maxhq/shared/i18n";
import { bellLabel } from "@/header/notifications";

import { ShortcutList } from "./sheet";

afterEach(() => {
	cleanup();
	setLanguageChoice("en");
});

it("updates rendered shortcut groups and descriptions when the language changes", () => {
	setLanguageChoice("en");
	render(<ShortcutList />);
	expect(screen.getByRole("region", { name: "General" })).toBeInTheDocument();
	expect(screen.getByText("Show keyboard shortcuts")).toBeInTheDocument();

	act(() => setLanguageChoice("nl"));
	expect(screen.getByRole("region", { name: "Algemeen" })).toBeInTheDocument();
	expect(screen.getByText("Sneltoetsen tonen")).toBeInTheDocument();
});

it("translates complete bell labels with singular and plural grammar", () => {
	setLanguageChoice("nl");
	expect(bellLabel(0)).toBe("Geen agent heeft je nodig");
	expect(bellLabel(1)).toBe("1 agent heeft je nodig");
	expect(bellLabel(3)).toBe("3 agents hebben je nodig");
});
