// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

import { PREFS_KEY } from "./prefs";

const onMac = () =>
	vi
		.spyOn(navigator, "userAgent", "get")
		.mockReturnValue("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)");

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
const field = () => screen.getByRole("searchbox", { name: "Filter agents" });
const group = (name: string) => within(nav()).queryByRole("region", { name });

// ⌥⌘F types ƒ on a Mac, so the key goes by where it sits.
const filterKey = () =>
	act(() => {
		fireEvent.keyDown(document.body, { code: "KeyF", key: "ƒ", metaKey: true, altKey: true });
	});

afterEach(() => {
	vi.restoreAllMocks();
});

describe("the sidebar's filter", () => {
	it("keeps the agents that hold every word, and Esc clears it, then leaves it", async () => {
		renderApp();
		await within(nav()).findByRole("region", { name: "Mosaic" });
		field().focus();

		fireEvent.change(field(), { target: { value: "subtitle DRIFT" } });
		const comet = group("Comet");
		expect(comet).not.toBeNull();
		expect(within(comet!).getByText("Subtitle sync drift")).toBeInTheDocument();
		expect(within(comet!).queryByText("HDR tone mapping")).toBeNull();
		expect(group("Mosaic")).toBeNull();
		expect(group("MaxHQ")).toBeNull();

		fireEvent.change(field(), { target: { value: "no such agent" } });
		expect(within(nav()).getByText("No agents match")).toBeInTheDocument();

		fireEvent.keyDown(field(), { key: "Escape" });
		expect(field()).toHaveValue("");
		expect(field()).toHaveFocus();
		expect(group("Mosaic")).not.toBeNull();
		expect(within(nav()).queryByText("No agents match")).toBeNull();

		fireEvent.keyDown(field(), { key: "Escape" });
		expect(field()).not.toHaveFocus();
	});

	it("opens the first agent shown on Enter, and does nothing while empty", async () => {
		const { ui } = renderApp();
		await within(nav()).findByRole("region", { name: "Comet" });
		const before = ui.getState().selectedAgent;

		fireEvent.keyDown(field(), { key: "Enter" });
		expect(ui.getState().selectedAgent).toBe(before);

		fireEvent.change(field(), { target: { value: "subtitle drift" } });
		fireEvent.keyDown(field(), { key: "Enter" });
		const row = within(group("Comet")!).getByText("Subtitle sync drift").closest("button");
		expect(row).toHaveAttribute("aria-current", "true");
		expect(ui.getState().selectedAgent).toBe(row!.getAttribute("data-agent-row"));
	});

	it("shows all of a project's agents when its name matches", async () => {
		renderApp();
		await within(nav()).findByRole("region", { name: "Comet" });
		fireEvent.change(field(), { target: { value: "comet" } });
		const comet = group("Comet")!;
		for (const title of ["HDR tone mapping", "Seek bar thumbnails", "Subtitle sync drift"]) {
			expect(within(comet).getByText(title)).toBeInTheDocument();
		}
		expect(group("Mosaic")).toBeNull();
	});

	it("focuses the field on ⌥⌘F, bringing back the sidebar and its projects", async () => {
		onMac();
		const { ui } = renderApp();
		await within(nav()).findByRole("region", { name: "Mosaic" });
		fireEvent.change(field(), { target: { value: "sync" } });
		act(() => ui.getState().setMode("devices"));
		fireEvent.click(screen.getByRole("button", { name: "Hide the sidebar" }));
		expect(screen.queryByRole("navigation", { name: "Devices" })).toBeNull();

		filterKey();
		expect(ui.getState().mode).toBe("projects");
		await waitFor(() => expect(field()).toHaveFocus());
		// What you typed stays while the window does, and is never saved.
		expect(field()).toHaveValue("sync");
		expect(localStorage.getItem(PREFS_KEY)).not.toContain("sync");
	});

	it("opens folded projects and closed shelves while it has words, then folds them back", async () => {
		renderApp();
		await within(nav()).findByRole("region", { name: "Comet" });
		const header = (name: string) => group(name)!.querySelector("button[aria-expanded]");
		fireEvent.click(header("Comet")!);
		expect(header("Comet")).toHaveAttribute("aria-expanded", "false");
		expect(within(group("Comet")!).queryByText("Subtitle sync drift")).toBeNull();

		fireEvent.change(field(), { target: { value: "subtitle" } });
		expect(header("Comet")).toHaveAttribute("aria-expanded", "true");
		expect(within(group("Comet")!).getByText("Subtitle sync drift")).toBeInTheDocument();

		fireEvent.change(field(), { target: { value: "skeleton" } });
		expect(header("Settled")).toHaveAttribute("aria-expanded", "true");
		expect(within(group("Settled")!).getByText(/M0 skeleton review/)).toBeInTheDocument();
		fireEvent.click(header("Settled")!);
		expect(header("Settled")).toHaveAttribute("aria-expanded", "false");
		expect(within(group("Settled")!).queryByText(/M0 skeleton review/)).toBeNull();

		fireEvent.keyDown(field(), { key: "Escape" });
		expect(header("Comet")).toHaveAttribute("aria-expanded", "false");
		expect(header("Settled")).toHaveAttribute("aria-expanded", "false");
		fireEvent.change(field(), { target: { value: "skeleton" } });
		expect(header("Settled")).toHaveAttribute("aria-expanded", "true");
	});
});
