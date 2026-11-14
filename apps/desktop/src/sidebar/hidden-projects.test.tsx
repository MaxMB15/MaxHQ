// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";

const renderApp = () => {
	const source = mockSource({ now: () => NOW, tick: false });
	const setHidden = vi.spyOn(source, "setProjectHidden");
	render(
		<FixedNow value={NOW}>
			<App source={source} />
		</FixedNow>
	);
	return { setHidden };
};

const nav = () => screen.getByRole("navigation", { name: "Agents" });
const project = (name: string) => within(nav()).queryByRole("region", { name });
const hiddenEntry = () => within(nav()).queryByRole("button", { name: /^Hidden projects/ });

// Picks an item from a project's menu.
const pick = async (name: string, item: string) => {
	await within(nav()).findByRole("region", { name });
	fireEvent.click(within(nav()).getByRole("button", { name: `More for ${name}` }));
	fireEvent.click(await screen.findByRole("menuitem", { name: item }));
};

afterEach(() => {
	vi.restoreAllMocks();
});

describe("hidden projects", () => {
	it("hides a quiet project and lists it under Hidden projects until you show it", async () => {
		const { setHidden } = renderApp();
		expect(await within(nav()).findByRole("region", { name: "Mosaic" })).toBeInTheDocument();
		expect(hiddenEntry()).toBeNull();

		await pick("Mosaic", "Hide project");
		expect(setHidden).toHaveBeenCalledWith("proj-mosaic", true);
		await waitFor(() => expect(project("Mosaic")).toBeNull());
		expect(hiddenEntry()).toHaveTextContent(/1$/);
		expect(hiddenEntry()).toHaveAttribute("aria-expanded", "false");

		fireEvent.click(hiddenEntry()!);
		const entry = within(nav()).getByRole("region", { name: "Hidden projects" });
		expect(entry).toHaveTextContent("Mosaic");
		fireEvent.click(within(entry).getByRole("button", { name: "Show project Mosaic" }));
		expect(setHidden).toHaveBeenLastCalledWith("proj-mosaic", false);
		await waitFor(() => expect(project("Mosaic")).toBeInTheDocument());
		expect(hiddenEntry()).toBeNull();
	});

	it("keeps a hidden project in view while an agent needs you, with Show project in its menu", async () => {
		const { setHidden } = renderApp();
		await pick("Comet", "Hide project");
		await waitFor(() => expect(hiddenEntry()).toHaveTextContent(/1$/));
		expect(project("Comet")).toBeInTheDocument();

		await pick("Comet", "Show project");
		expect(setHidden).toHaveBeenLastCalledWith("proj-comet", false);
		await waitFor(() => expect(hiddenEntry()).toBeNull());
	});

	it("says why a hide didn't go through", async () => {
		const { setHidden } = renderApp();
		setHidden.mockResolvedValueOnce({
			status: "ok",
			data: { status: "refused", reason: "that list is on another device" },
		});
		await pick("Mosaic", "Hide project");
		expect(await within(nav()).findByRole("alert")).toHaveTextContent(
			"Couldn't hide Mosaic. That list is on another device."
		);
		expect(project("Mosaic")).toBeInTheDocument();
	});
});
