// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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
const form = () => screen.getByRole("dialog", { name: "New agent" });
const project = () => within(form()).getByRole("combobox", { name: /^Project,/ });

const closeMenu = async () => {
	fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
	await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());
};

describe("a project's menu", () => {
	it("starts with New agent and offers New note, from the right-click and the ⋯", async () => {
		renderApp();
		const mosaic = await within(nav()).findByRole("region", { name: "Mosaic" });
		const first = async () => (await screen.findAllByRole("menuitem"))[0]?.textContent;

		fireEvent.contextMenu(within(mosaic).getByRole("button", { name: /^Mosaic/ }));
		expect(await first()).toBe("New agent");
		expect(screen.getByRole("menuitem", { name: "New note" })).toBeInTheDocument();
		await closeMenu();
		fireEvent.click(within(nav()).getByRole("button", { name: "More for Mosaic" }));
		expect(await first()).toBe("New agent");
		expect(screen.getByRole("menuitem", { name: "New note" })).toBeInTheDocument();
	});

	it("opens the new-agent form for the project, as its + does", async () => {
		const { ui } = renderApp();
		const mosaic = await within(nav()).findByRole("region", { name: "Mosaic" });
		// "Subtitle sync drift" works in Comet, so the form doesn't fall back to it.
		act(() => ui.getState().selectAgent("agent-sync-drift", false));

		fireEvent.contextMenu(within(mosaic).getByRole("button", { name: /^Mosaic/ }));
		fireEvent.click(await screen.findByRole("menuitem", { name: "New agent" }));
		expect(project()).toHaveTextContent("Mosaic");
		fireEvent.keyDown(form(), { key: "Escape" });
		await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());

		fireEvent.click(within(nav()).getByRole("button", { name: "New agent in Mosaic" }));
		expect(project()).toHaveTextContent("Mosaic");
	});
});
