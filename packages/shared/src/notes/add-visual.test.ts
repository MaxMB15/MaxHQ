// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";

import type { NoteVersion, ProjectId } from "../bindings";
import type { DataSource, NoteResult, NoteSave } from "../data/source";
import { agent, entities, project, workspace } from "../test/build";
import type { NotesReach } from "../view/notes";
import { applySnapshot, type ViewState } from "../view/replica";
import { toVisualFence } from "../visuals/fence";
import { MAX_VISUAL_BYTES } from "../visuals/visual";

import { addToHead, addToNewNote, addToNoteTarget, takeOutOfHead } from "./add-visual";

const WRITE: NotesReach = { device: "dev-box", readOnly: false, why: null };
const READ: NotesReach = { device: "dev-box", readOnly: true, why: "Read only, for a test." };

const visual = { html: "<p>hi</p>", title: "Hi" };
const fence = toVisualFence(visual);

/** A data source whose one note starts as `body`, and takes each save as its new head. */
const sourceOf = (body: string) => {
	let head: NoteVersion = {
		hash: "h1",
		project: "p-a",
		note: "n1",
		parents: [],
		title: "Plan",
		body,
		deleted: false,
		at: 0,
		by: null,
	};
	const saves: { note: string; save: NoteSave }[] = [];
	const source = {
		readNote: vi.fn(async (): Promise<NoteResult> => ({ status: "ok", data: head })),
		saveNote: vi.fn(async (_device: string, _project: string, note: string, save: NoteSave) => {
			saves.push({ note, save });
			head = { ...head, hash: `h${saves.length + 1}`, body: save.body };
			return { status: "ok" as const, data: { version: head, merged: false } };
		}),
	} as unknown as DataSource;
	return { source, saves, body: () => head.body };
};

const view = applySnapshot({
	seq: 1,
	entities: entities({
		projects: [project("p-a", "Atlas"), project("p-b", "Birch")],
		workspaces: [workspace({ id: "w1", project: "p-a" })],
		agents: [agent({ id: "a1", workspace: "w1" })],
	}),
});

describe("addToNoteTarget", () => {
	const writable = () => WRITE;

	it("offers the agent's project first, else the note's", () => {
		expect(addToNoteTarget(view, { agent: "a1", project: "p-b" }, writable)).toEqual({
			project: "p-a",
			reach: WRITE,
		});
		expect(addToNoteTarget(view, { agent: "nobody", project: "p-b" }, writable)?.project).toBe(
			"p-b"
		);
		expect(addToNoteTarget(view, { agent: null, project: null }, writable)).toBeNull();
	});

	it("offers none where the notes are read-only", () => {
		const reachOf = vi.fn<(view: ViewState, project: ProjectId) => NotesReach>(() => READ);
		expect(addToNoteTarget(view, { agent: "a1", project: null }, reachOf)).toBeNull();
		expect(reachOf).toHaveBeenCalledWith(view, "p-a");
	});
});

describe("addToHead", () => {
	it("puts the visual at the end of the head, and Undo takes it out", async () => {
		const { source, body } = sourceOf("one\n");
		const added = await addToHead(source, WRITE, "p-a", "n1", visual);
		expect(body()).toBe(`one\n\n${fence}\n`);
		if (!("placed" in added)) throw new Error(added.failure);
		expect(await takeOutOfHead(source, WRITE, "p-a", "n1", added.placed)).toBeNull();
		expect(body()).toBe("one\n");
	});

	it("says why a visual too large for a note stays out", async () => {
		const { source, saves } = sourceOf("");
		const added = await addToHead(source, WRITE, "p-a", "n1", {
			html: "x".repeat(MAX_VISUAL_BYTES + 1),
			title: "Big",
		});
		expect(added).toEqual({ failure: expect.stringContaining("at most") });
		expect(saves).toEqual([]);
	});

	it("says why without a device that can write", async () => {
		const { source } = sourceOf("");
		expect(await addToHead(source, READ, "p-a", "n1", visual)).toEqual({
			failure: "Read only, for a test.",
		});
	});
});

describe("takeOutOfHead", () => {
	it("leaves a visual that changed since it went in", async () => {
		const { source, body } = sourceOf("one\n");
		const added = await addToHead(source, WRITE, "p-a", "n1", visual);
		if (!("placed" in added)) throw new Error(added.failure);
		// Someone edits the visual's block after it went in.
		await source.saveNote("dev-box", "p-a", "n1", {
			base: null,
			title: "Plan",
			body: body().replace("<p>hi</p>", "<p>hey</p>"),
			deleted: false,
		});
		expect(await takeOutOfHead(source, WRITE, "p-a", "n1", added.placed)).toBe(
			"The visual changed since it went in, so it stays."
		);
	});
});

describe("addToNewNote", () => {
	it("makes a note holding only the visual", async () => {
		const { source, saves } = sourceOf("");
		const made = await addToNewNote(source, view, WRITE, "p-a", visual);
		if (!("note" in made)) throw new Error(made.failure);
		expect(saves).toEqual([
			{
				note: made.note,
				save: { base: null, title: "Untitled", body: `${fence}\n`, deleted: false },
			},
		]);
		expect(made.placed).toEqual({ at: 0, text: `${fence}\n` });
	});

	it("says why it couldn't", async () => {
		const { source } = sourceOf("");
		expect(await addToNewNote(source, view, READ, "p-a", visual)).toEqual({
			failure: "Read only, for a test.",
		});
	});
});
