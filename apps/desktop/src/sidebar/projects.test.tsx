// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

const renderApp = () => {
	const ui = createUiStore();
	render(
		<FixedNow value={NOW}>
			<App source={mockSource({ now: () => NOW, tick: false })} ui={ui} />
		</FixedNow>
	);
	return { ui };
};

const nav = () => screen.getByRole("navigation", { name: "Agents" });
const more = (name: string) => within(nav()).getByRole("button", { name: `More for ${name}` });
const add = (name: string) => within(nav()).getByRole("button", { name: `New agent in ${name}` });

describe("the agent list", () => {
	it("moves between the agents with Up and Down, across projects and shelves", async () => {
		const { ui } = renderApp();
		const list = await within(nav()).findByRole("listbox", { name: "Agents" });
		expect(list).toHaveAttribute("aria-multiselectable", "true");
		const settled = within(nav()).getByRole("region", { name: "Settled" });
		fireEvent.click(within(settled).getByRole("button", { name: /^Settled/ }));
		// The list hears of the new rows once the DOM settles.
		await act(() => Promise.resolve());

		const rows = within(list).getAllByRole("option");
		expect(rows.filter((row) => row.tabIndex === 0)).toHaveLength(1);
		const sections = new Set(rows.map((row) => row.closest("section")));
		expect(sections.size).toBeGreaterThan(2);
		act(() => rows[0]!.focus());
		for (const row of rows.slice(1)) {
			fireEvent.keyDown(document.activeElement!, { key: "ArrowDown" });
			expect(row).toHaveFocus();
		}
		expect(settled).toContainElement(document.activeElement as HTMLElement);
		// The ring sits inside the row, where the list's edges can't clip it.
		expect(document.activeElement).toHaveClass("focus-visible:-outline-offset-2");

		// A row is a button, so Enter opens its agent as a click does.
		const at = document.activeElement as HTMLElement;
		expect(at.tagName).toBe("BUTTON");
		fireEvent.click(at);
		expect(ui.getState().selectedAgent).toBe(at.dataset.agentRow);
	});
});

describe("a project's header", () => {
	it("opens the project's settings from its menu", async () => {
		renderApp();
		await within(nav()).findByRole("region", { name: "Mosaic" });
		fireEvent.click(more("Mosaic"));
		fireEvent.click(await screen.findByRole("menuitem", { name: "Project settings…" }));
		const dialog = await screen.findByRole("dialog", { name: "Settings" });
		const pages = within(dialog).getByRole("navigation", { name: "Settings pages" });
		expect(within(pages).getByRole("button", { name: "Mosaic" })).toHaveAttribute(
			"aria-current",
			"page"
		);
	});

	it("keeps its + and ⋯ showing on the project of the agent you're looking at", async () => {
		const { ui } = renderApp();
		await within(nav()).findByRole("region", { name: "Comet" });
		for (const button of [more("Comet"), add("Comet"), more("Mosaic")]) {
			expect(button).toHaveClass("opacity-0", "group-hover/header:opacity-100");
		}

		// "Subtitle sync drift" works in Comet.
		act(() => ui.getState().selectAgent("agent-sync-drift", false));
		expect(more("Comet")).toHaveClass("opacity-100");
		expect(more("Comet")).not.toHaveClass("opacity-0");
		expect(add("Comet")).toHaveClass("opacity-100");
		expect(more("Mosaic")).toHaveClass("opacity-0");
	});
});
