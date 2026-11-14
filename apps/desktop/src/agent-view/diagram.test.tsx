// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import globals from "@/styles/globals.css?raw";

import { fnv1a } from "@maxhq/shared/lib/fnv";

const SOURCE = "flowchart LR\n a --> b";

// Mermaid works out its colors from the theme.
for (const [, name, value] of globals.matchAll(/(--[\w-]+):\s*([^;]+);/g))
	if (name && value) document.documentElement.style.setProperty(name, value);

// As a window that went away in the middle of drawing it leaves things.
const leftDrawing = async (source: string) => {
	localStorage.setItem("maxhq.diagram.drawing", fnv1a(source).toString(36));
	vi.resetModules();
	return import("./diagram");
};

beforeEach(() => localStorage.clear());
afterEach(cleanup);

describe("a diagram that was drawing when the window went away", () => {
	it("shows its source and why, and draws only when asked", async () => {
		const { DiagramPreview } = await leftDrawing(SOURCE);
		render(<DiagramPreview source={SOURCE} />);

		const button = await screen.findByRole("button", { name: "Draw anyway" });
		expect(screen.getByText(/stopped or closed while drawing it/)).toBeTruthy();

		fireEvent.click(button);
		await waitFor(() => expect(screen.queryByText(/stopped or closed/)).toBeNull(), {
			timeout: 30_000,
		});
		expect(screen.queryByRole("button", { name: "Draw anyway" })).toBeNull();
		expect(localStorage.getItem("maxhq.diagram.heldBack")).toBe("");
	}, 40_000);

	it("offers nothing for a diagram that failed for another reason", async () => {
		vi.resetModules();
		const { DiagramPreview } = await import("./diagram");
		render(<DiagramPreview source={"flowchart LR\n a -->"} />);

		await screen.findByText(/Couldn't draw this diagram/, undefined, { timeout: 30_000 });
		expect(screen.queryByRole("button", { name: "Draw anyway" })).toBeNull();
	}, 40_000);
});

// The test page can't measure text, so the drawing ends as a failure, which
// still shows it was drawn in the new theme and not left drawing.
it("draws again when the theme changes while drawing", async () => {
	vi.resetModules();
	const { DiagramPreview } = await import("./diagram");
	const { setThemeChoice } = await import("@/app/theme");
	setThemeChoice("dark");
	render(<DiagramPreview source={SOURCE} />);
	setThemeChoice("light");

	await screen.findByText(/Couldn't draw this diagram/, undefined, { timeout: 30_000 });
	expect(screen.queryByRole("status", { name: "Drawing the diagram" })).toBeNull();
}, 40_000);
