// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";

import type { NoteVersion } from "../bindings";
import type { DataSource, NoteResult, NoteSave, SaveNoteResult } from "../data/source";
import { entities, project } from "../test/build";
import type { NotesReach } from "../view/notes";
import { applySnapshot } from "../view/replica";

import { changeHead, makeNote, newNoteId, writerOf } from "./actions";

const WRITE: NotesReach = { device: "dev-box", readOnly: false, why: null };
const READ: NotesReach = { device: "dev-box", readOnly: true, why: "Read only, for a test." };
const SILENT: NotesReach = { device: "dev-box", readOnly: true, why: null };

const head: NoteVersion = {
	hash: "h1",
	project: "p-a",
	note: "n1",
	parents: [],
	title: "Plan",
	body: "one\n",
	deleted: false,
	at: 0,
	by: null,
};

/** A data source that answers note reads with `head` and keeps each save. */
const sourceOf = (
	answer: SaveNoteResult = { status: "ok", data: { version: head, merged: false } }
) => {
	const saves: { device: string; note: string; save: NoteSave }[] = [];
	const source = {
		readNote: vi.fn(async (): Promise<NoteResult> => ({ status: "ok", data: head })),
		saveNote: vi.fn(async (device: string, _project: string, note: string, save: NoteSave) => {
			saves.push({ device, note, save });
			return answer;
		}),
	} as unknown as DataSource;
	return { source, saves };
};

const view = applySnapshot({ seq: 1, entities: entities({ projects: [project("p-a", "Atlas")] }) });

describe("writerOf", () => {
	it("gives the device only while it can write", () => {
		expect(writerOf(WRITE)).toEqual({ device: "dev-box", why: null });
		expect(writerOf(READ)).toEqual({ device: null, why: "Read only, for a test." });
		expect(writerOf(SILENT)).toEqual({ device: null, why: "These notes are read-only here." });
	});
});

describe("newNoteId", () => {
	it("makes a version 4 UUID, each one new", () => {
		const shape = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
		expect(newNoteId()).toMatch(shape);
		expect(newNoteId()).not.toBe(newNoteId());
	});
});

describe("makeNote", () => {
	it("saves an empty Untitled note with no base", async () => {
		const { source, saves } = sourceOf();
		const made = await makeNote(source, view, WRITE, "p-a");
		expect("note" in made).toBe(true);
		expect(saves).toEqual([
			{
				device: "dev-box",
				note: "note" in made ? made.note : "",
				save: { base: null, title: "Untitled", body: "", deleted: false },
			},
		]);
	});

	it("saves the text it's given", async () => {
		const { source, saves } = sourceOf();
		await makeNote(source, view, WRITE, "p-a", "a visual\n");
		expect(saves[0]?.save.body).toBe("a visual\n");
	});

	it("says why without a device that can write", async () => {
		const { source, saves } = sourceOf();
		expect(await makeNote(source, view, READ, "p-a")).toEqual({
			failure: "Read only, for a test.",
		});
		expect(saves).toEqual([]);
	});

	it("says why the device refused", async () => {
		const { source } = sourceOf({
			status: "error",
			error: { kind: "failed", message: "No room." },
		});
		expect(await makeNote(source, view, WRITE, "p-a")).toEqual({ failure: "No room." });
	});
});

describe("changeHead", () => {
	it("saves the change as a new version on the head", async () => {
		const { source, saves } = sourceOf();
		expect(await changeHead(source, WRITE, "p-a", "n1", { deleted: true })).toBeNull();
		expect(saves.map((s) => s.save)).toEqual([
			{ base: "h1", title: "Plan", body: "one\n", deleted: true },
		]);
		await changeHead(source, WRITE, "p-a", "n1", { title: "Plan B" });
		expect(saves.at(-1)?.save).toEqual({
			base: "h1",
			title: "Plan B",
			body: "one\n",
			deleted: false,
		});
	});

	it("changes the head's text, or says why it can't", async () => {
		const { source, saves } = sourceOf();
		expect(
			await changeHead(source, WRITE, "p-a", "n1", { body: (body) => `${body}two\n` })
		).toBeNull();
		expect(saves.at(-1)?.save.body).toBe("one\ntwo\n");
		expect(
			await changeHead(source, WRITE, "p-a", "n1", { body: () => ({ refused: "Too big." }) })
		).toBe("Too big.");
		expect(saves).toHaveLength(1);
	});

	it("says why without a device that can write", async () => {
		const { source, saves } = sourceOf();
		expect(await changeHead(source, SILENT, "p-a", "n1", { deleted: true })).toBe(
			"These notes are read-only here."
		);
		expect(saves).toEqual([]);
	});
});
