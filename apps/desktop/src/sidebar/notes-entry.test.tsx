// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import { hideAdded } from "@/visuals/added-toast";
import { toVisualFence } from "@maxhq/shared/visuals/fence";
import { VISUAL_MIME, writeVisualTransfer } from "@maxhq/shared/visuals/transfer";
import type { DataSource } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

const renderApp = () => {
	const source: DataSource = mockSource({ now: () => NOW, tick: false });
	const saveNote = vi.spyOn(source, "saveNote");
	render(
		<FixedNow value={NOW}>
			<App source={source} ui={createUiStore()} />
		</FixedNow>
	);
	return { saveNote };
};

const nav = () => screen.getByRole("navigation", { name: "Agents" });
const notesRow = () => within(nav()).findByRole("button", { name: /^Notes/ });

afterEach(() => {
	vi.restoreAllMocks();
	localStorage.clear();
});

describe("a project's notes in the sidebar", () => {
	it("lists them under a row that folds, with how many", async () => {
		renderApp();
		const row = await notesRow();
		expect(row).toHaveTextContent("2");
		expect(row).toHaveAttribute("aria-expanded", "true");
		expect(within(nav()).getByRole("button", { name: "Design notes" })).toBeInTheDocument();

		fireEvent.click(row);
		expect(row).toHaveAttribute("aria-expanded", "false");
		expect(within(nav()).queryByRole("button", { name: "Design notes" })).toBeNull();
	});

	it("opens a note's pane on a click", async () => {
		renderApp();
		await notesRow();
		fireEvent.click(within(nav()).getByRole("button", { name: "GTFS release steps" }));
		expect(await screen.findByRole("textbox", { name: "Title" })).toHaveValue(
			"GTFS release steps"
		);
	});

	it("deletes a note once you say so", async () => {
		const { saveNote } = renderApp();
		await notesRow();
		fireEvent.contextMenu(within(nav()).getByRole("button", { name: "Design notes" }));
		fireEvent.click(await screen.findByRole("menuitem", { name: /Delete/ }));
		const dialog = await screen.findByRole("alertdialog");
		expect(dialog).toHaveTextContent('Delete note "Design notes"?');
		fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));

		await waitFor(() =>
			expect(within(nav()).queryByRole("button", { name: "Design notes" })).toBeNull()
		);
		expect(saveNote.mock.calls.at(-1)?.[3]).toMatchObject({ deleted: true });
	});

	it("creates a note from a right-click on the Notes row or on a note", async () => {
		const { saveNote } = renderApp();
		const made = async (count: number) => {
			fireEvent.click(await screen.findByRole("menuitem", { name: "New note" }));
			await waitFor(() => expect(saveNote).toHaveBeenCalledTimes(count));
			expect(saveNote.mock.calls.at(-1)?.[3]).toMatchObject({ base: null, body: "" });
			expect(await screen.findByRole("textbox", { name: "Title" })).toHaveValue(
				count === 1 ? "Untitled" : "Untitled 2"
			);
		};

		fireEvent.contextMenu(await notesRow());
		await made(1);
		fireEvent.contextMenu(within(nav()).getByRole("button", { name: "Design notes" }));
		await made(2);
	});
});

describe("a visual dropped on a note in the sidebar", () => {
	const VISUAL = { html: "<p>chart</p>", title: "Chart" };
	const FENCE = toVisualFence(VISUAL);
	const dataTransfer = {
		types: [VISUAL_MIME],
		getData: (type: string) =>
			type === VISUAL_MIME ? writeVisualTransfer({ ...VISUAL, from: "Atlas on devbox" }) : "",
		files: [],
		dropEffect: "none",
	};
	const drop = (row: HTMLElement) => {
		fireEvent.dragOver(row, { dataTransfer });
		expect(row).toHaveAttribute("data-drop-over");
		fireEvent.drop(row, { dataTransfer });
		expect(row).not.toHaveAttribute("data-drop-over");
	};

	afterEach(() => act(hideAdded));

	/** The toast, among the other things the app says politely. */
	const toast = async () => (await screen.findByText("Added to")).closest("[role=status]");

	it("goes in at the end of a note that isn't open, and Undo takes it out", async () => {
		const { saveNote } = renderApp();
		await notesRow();
		drop(within(nav()).getByRole("button", { name: "Design notes" }));

		await waitFor(() => expect(saveNote).toHaveBeenCalledTimes(1));
		const added = saveNote.mock.calls[0]?.[3].body ?? "";
		expect(added.endsWith(`${FENCE}\n`)).toBe(true);
		expect(await toast()).toHaveTextContent("Design notes");

		fireEvent.click(screen.getByRole("button", { name: "Undo" }));
		await waitFor(() => expect(saveNote).toHaveBeenCalledTimes(2));
		expect(saveNote.mock.calls[1]?.[3].body).toBe(added.replace(`\n${FENCE}\n`, ""));
	});

	it("goes in at the caret of a note open in a pane", async () => {
		renderApp();
		await notesRow();
		const row = within(nav()).getByRole("button", { name: "GTFS release steps" });
		fireEvent.click(row);
		const note = await screen.findByRole("textbox", { name: "Note" });
		drop(row);
		expect(await within(note).findByRole("figure", { name: "Chart" })).toBeInTheDocument();
		expect(await toast()).toHaveTextContent("GTFS release steps");

		fireEvent.click(screen.getByRole("button", { name: "Undo" }));
		await waitFor(() =>
			expect(within(note).queryByRole("figure", { name: "Chart" })).toBeNull()
		);
	});
});
