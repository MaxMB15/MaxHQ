// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import type { ActionResult, DataSource } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

const renderApp = () => {
	const source: DataSource = mockSource({ now: () => NOW, tick: false });
	const renameAgent = vi.spyOn(source, "renameAgent");
	render(
		<FixedNow value={NOW}>
			<App source={source} ui={createUiStore()} />
		</FixedNow>
	);
	return { renameAgent };
};

const nav = () => screen.getByRole("navigation", { name: "Agents" });
// An agent's row in the sidebar's list, not the Needs you box, which has no menu.
const row = (name: RegExp) => within(nav()).findByRole("option", { name });
const field = () => screen.findByRole<HTMLInputElement>("textbox", { name: "Agent name" });

const renameFromMenu = async (name: RegExp) => {
	fireEvent.contextMenu(await row(name));
	const menu = await screen.findByRole("menu");
	fireEvent.click(within(menu).getByRole("menuitem", { name: "Rename" }));
	return field();
};

const renameByDoubleClick = async (title: string) => {
	fireEvent.doubleClick(within(await row(new RegExp(title))).getByText(title));
	return field();
};

const type = (input: HTMLInputElement, value: string) =>
	fireEvent.change(input, { target: { value } });

// An answer the test gives when it's ready.
const later = () => {
	let answer!: (result: ActionResult) => void;
	const promise = new Promise<ActionResult>((resolve) => (answer = resolve));
	return { promise, answer };
};

afterEach(() => {
	vi.restoreAllMocks();
});

describe("renaming an agent in the sidebar", () => {
	it("opens from the row's menu with the title selected, and Enter saves", async () => {
		const { renameAgent } = renderApp();
		const input = await renameFromMenu(/Seek bar thumbnails/);
		expect(input).toHaveFocus();
		expect(input.value).toBe("Seek bar thumbnails");
		expect([input.selectionStart, input.selectionEnd]).toEqual([0, input.value.length]);

		type(input, "  Thumbnail strip ");
		fireEvent.keyDown(input, { key: "Enter" });
		expect(renameAgent).toHaveBeenCalledWith("agent-cursor-seek", "Thumbnail strip");
		expect(await row(/Thumbnail strip/)).toBeInTheDocument();
		expect(screen.queryByRole("textbox", { name: "Agent name" })).toBeNull();
	});

	it("opens on a double-click on the title, and leaving the field saves", async () => {
		const { renameAgent } = renderApp();
		const input = await renameByDoubleClick("Bindings check");
		expect(input).toHaveFocus();
		type(input, "Bindings drift");
		fireEvent.blur(input);
		expect(renameAgent).toHaveBeenCalledWith("agent-bindings", "Bindings drift");
		expect(await row(/Bindings drift/)).toBeInTheDocument();
	});

	it("leaves a double-click with a modifier to the selection", async () => {
		renderApp();
		fireEvent.doubleClick(within(await row(/Bindings check/)).getByText("Bindings check"), {
			metaKey: true,
		});
		expect(screen.queryByRole("textbox", { name: "Agent name" })).toBeNull();
	});

	it("puts the title back on Escape and sends nothing", async () => {
		const { renameAgent } = renderApp();
		const input = await renameByDoubleClick("Seek bar thumbnails");
		type(input, "Something else");
		fireEvent.keyDown(input, { key: "Escape" });
		expect(screen.queryByRole("textbox", { name: "Agent name" })).toBeNull();
		expect(await row(/Seek bar thumbnails/)).toBeInTheDocument();
		expect(renameAgent).not.toHaveBeenCalled();
	});

	it("sends nothing when the title stays as it was", async () => {
		const { renameAgent } = renderApp();
		const input = await renameFromMenu(/Seek bar thumbnails/);
		fireEvent.keyDown(input, { key: "Enter" });
		expect(renameAgent).not.toHaveBeenCalled();
	});

	it("brings the agent's own title back when you clear the name", async () => {
		const { renameAgent } = renderApp();
		let input = await renameFromMenu(/Seek bar thumbnails/);
		type(input, "Thumbnail strip");
		fireEvent.keyDown(input, { key: "Enter" });
		await row(/Thumbnail strip/);

		input = await renameFromMenu(/Thumbnail strip/);
		type(input, "   ");
		fireEvent.keyDown(input, { key: "Enter" });
		expect(renameAgent).toHaveBeenLastCalledWith("agent-cursor-seek", "");
		expect(await row(/Seek bar thumbnails/)).toBeInTheDocument();
	});

	it("shows the name at once and puts the title back when maxd refuses", async () => {
		const { renameAgent } = renderApp();
		const { promise, answer } = later();
		renameAgent.mockReturnValue(promise);
		const input = await renameFromMenu(/Seek bar thumbnails/);
		type(input, "Thumbnail strip");
		fireEvent.keyDown(input, { key: "Enter" });
		expect(await row(/Thumbnail strip/)).toBeInTheDocument();

		answer({ status: "ok", data: { status: "refused", reason: "couldn't store the name" } });
		expect(await within(nav()).findByRole("alert")).toHaveTextContent(
			"Couldn't rename Seek bar thumbnails. Couldn't store the name."
		);
		expect(await row(/Seek bar thumbnails/)).toBeInTheDocument();
		expect(within(nav()).queryByRole("option", { name: /Thumbnail strip/ })).toBeNull();
	});

	it("isn't offered for several agents at once", async () => {
		renderApp();
		fireEvent.click(await row(/Seek bar thumbnails/), { ctrlKey: true });
		fireEvent.click(await row(/Bindings check/), { ctrlKey: true });
		fireEvent.contextMenu(await row(/Bindings check/));
		const menu = await screen.findByRole("menu");
		expect(within(menu).queryByRole("menuitem", { name: "Rename" })).toBeNull();
	});
});

describe("renaming an agent in its header", () => {
	const openSeekBar = async () => {
		fireEvent.click(await row(/Seek bar thumbnails/));
		return screen.findByRole("heading", { level: 1, name: "Seek bar thumbnails" });
	};

	it("turns the title into a field on a double-click, and the sidebar follows", async () => {
		const { renameAgent } = renderApp();
		fireEvent.doubleClick(await openSeekBar());
		const input = await field();
		expect(input).toHaveFocus();
		expect([input.selectionStart, input.selectionEnd]).toEqual([0, input.value.length]);
		type(input, "Thumbnail strip");
		fireEvent.keyDown(input, { key: "Enter" });

		expect(renameAgent).toHaveBeenCalledWith("agent-cursor-seek", "Thumbnail strip");
		expect(
			await screen.findByRole("heading", { level: 1, name: "Thumbnail strip" })
		).toBeInTheDocument();
		expect(await row(/Thumbnail strip/)).toBeInTheDocument();
	});

	it("puts the title back on Escape", async () => {
		const { renameAgent } = renderApp();
		fireEvent.doubleClick(await openSeekBar());
		const input = await field();
		type(input, "Thumbnail strip");
		fireEvent.keyDown(input, { key: "Escape" });
		expect(
			await screen.findByRole("heading", { level: 1, name: "Seek bar thumbnails" })
		).toBeInTheDocument();
		expect(renameAgent).not.toHaveBeenCalled();
	});

	it("says below the strip why a rename failed and puts the title back", async () => {
		const { renameAgent } = renderApp();
		renameAgent.mockRejectedValue(new Error("devbox isn't connected"));
		fireEvent.doubleClick(await openSeekBar());
		const input = await field();
		type(input, "Thumbnail strip");
		fireEvent.blur(input);

		const why = await screen.findByText(
			"Couldn't rename Seek bar thumbnails. Devbox isn't connected."
		);
		expect(why).toHaveAttribute("role", "alert");
		await waitFor(() =>
			expect(screen.getByRole("heading", { level: 1 })).toHaveAccessibleName(
				"Seek bar thumbnails"
			)
		);
	});
});
