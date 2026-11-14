// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { type Services, ServicesProvider, useServices } from "@/app/services";
import { mockSource } from "@/data/mock-source";
import { DESIGN, NOTES_PROJECT } from "@maxhq/shared/data/mock-notes";
import type { DataSource } from "@maxhq/shared/data/source";
import { notesOf } from "@maxhq/shared/view/notes";
import { toVisualFence } from "@maxhq/shared/visuals/fence";
import { NOW } from "@maxhq/shared/test/build";

import { menuEntries } from "./actions";
import { addToNoteAction } from "./add-to-note";
import { hideAdded } from "./added-toast";

const VISUAL = { html: "<p>chart</p>", title: "Chart" };
const FENCE = toVisualFence(VISUAL);

const start = async () => {
	const source: DataSource = mockSource({ now: () => NOW, tick: false });
	const saveNote = vi.spyOn(source, "saveNote");
	const held: { services: Services | null } = { services: null };
	const Probe = () => {
		held.services = useServices();
		return null;
	};
	render(
		<ServicesProvider source={source}>
			<Probe />
		</ServicesProvider>
	);
	const services = held.services!;
	await waitFor(() =>
		expect(notesOf(services.view.getState().view, NOTES_PROJECT).length).toBe(2)
	);
	return { services, source, saveNote };
};

const headOf = async (source: DataSource) => {
	const read = await source.readNote("dev-mac", NOTES_PROJECT, DESIGN, null);
	if (read.status !== "ok") throw new Error("no note");
	return read.data;
};

const bodyOf = async (source: DataSource) => (await headOf(source)).body;

afterEach(() => {
	act(hideAdded);
	vi.restoreAllMocks();
});

describe("Add to note", () => {
	it("lists the notes of the agent's project, then New note", async () => {
		const { services } = await start();
		const action = addToNoteAction(services, { agent: "agent-gtfs", project: null }, VISUAL);
		const entries = action.entries?.() ?? [];
		expect(entries.map((entry) => entry.label)).toEqual([
			...notesOf(services.view.getState().view, NOTES_PROJECT).map((row) => row.shown),
			"New note",
		]);
		expect(entries.at(-1)?.separated).toBe(true);
	});

	it("puts a line before New note in the system menu, and dims an empty list", async () => {
		const { services } = await start();
		const action = addToNoteAction(services, { agent: "agent-gtfs", project: null }, VISUAL);
		const [menu] = menuEntries([action]);
		expect(menu).toMatchObject({ kind: "submenu", text: "Add to note" });
		const entries = menu?.kind === "submenu" ? menu.entries : [];
		expect(entries.at(-2)).toEqual({ kind: "separator" });
		expect(entries.at(-1)).toMatchObject({ kind: "item", text: "New note" });
		const lost = addToNoteAction(services, { agent: "agent-gone", project: null }, VISUAL);
		expect(menuEntries([lost])).toEqual([
			{ kind: "submenu", text: "Add to note", entries: [] },
		]);
	});

	it("lists the notes of the note's own project, and none for an unknown agent", async () => {
		const { services } = await start();
		const fromNote = addToNoteAction(services, { agent: null, project: NOTES_PROJECT }, VISUAL);
		expect(fromNote.entries?.()).toHaveLength(3);
		const lost = addToNoteAction(services, { agent: "agent-gone", project: null }, VISUAL);
		expect(lost.entries?.()).toEqual([]);
	});

	it("puts the visual at the end of a note that isn't open, and Undo takes it out", async () => {
		const { services, source, saveNote } = await start();
		const before = await bodyOf(source);
		const action = addToNoteAction(services, { agent: "agent-gtfs", project: null }, VISUAL);
		act(() =>
			action
				.entries?.()
				.find((entry) => entry.label === "Design notes")
				?.run()
		);

		const toast = await screen.findByRole("status");
		expect(toast).toHaveTextContent("Added to");
		expect(toast).toHaveTextContent("Design notes");
		await waitFor(() => expect(saveNote).toHaveBeenCalledTimes(1));
		const added = await bodyOf(source);
		expect(added.startsWith(before)).toBe(true);
		expect(added.endsWith(`${FENCE}\n`)).toBe(true);

		fireEvent.click(screen.getByRole("button", { name: "Undo" }));
		await waitFor(() => expect(saveNote).toHaveBeenCalledTimes(2));
		expect(await bodyOf(source)).toBe(before);
	});

	it("leaves a block that changed since it went in", async () => {
		const { services, source, saveNote } = await start();
		const action = addToNoteAction(services, { agent: null, project: NOTES_PROJECT }, VISUAL);
		act(() =>
			action
				.entries?.()
				.find((entry) => entry.label === "Design notes")
				?.run()
		);
		await screen.findByRole("status");
		await waitFor(() => expect(saveNote).toHaveBeenCalledTimes(1));
		const head = await headOf(source);
		await source.saveNote("dev-mac", NOTES_PROJECT, DESIGN, {
			base: head.hash,
			title: head.title,
			body: head.body.replace("<p>chart</p>", "<p>changed</p>"),
			deleted: false,
		});

		fireEvent.click(screen.getByRole("button", { name: "Undo" }));
		expect(
			await screen.findByText("The visual changed since it went in, so it stays.")
		).toBeInTheDocument();
	});

	it("makes a new note holding only the visual", async () => {
		const { services, saveNote } = await start();
		const action = addToNoteAction(services, { agent: "agent-gtfs", project: null }, VISUAL);
		act(() => action.entries?.().at(-1)?.run());
		await waitFor(() => expect(saveNote).toHaveBeenCalledTimes(1));
		expect(saveNote.mock.calls[0]?.[3]).toMatchObject({ base: null, body: `${FENCE}\n` });
		expect(await screen.findByRole("status")).toHaveTextContent("Untitled");
	});
});
