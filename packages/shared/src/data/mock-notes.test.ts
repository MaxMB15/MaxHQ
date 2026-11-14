// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";

import { MAX_NOTE_TITLE, type NoteVersion, type ProjectNotes } from "../bindings";
import { NOW } from "../test/build";
import { CHECKLIST, DESIGN, NOTES_PROJECT, mockNotes } from "./mock-notes";
import { DEVBOX, MAC } from "./mock-view";

const setup = () => {
	const changed = vi.fn();
	const notes = mockNotes({ now: () => NOW, changed });
	return { notes, changed };
};

const ok = <T>(result: { status: "ok"; data: T } | { status: "error" }): T => {
	if (result.status !== "ok") throw new Error(JSON.stringify(result));
	return result.data;
};

const head = async (
	notes: ReturnType<typeof setup>["notes"],
	note = CHECKLIST
): Promise<NoteVersion> => ok(await notes.calls.readNote(MAC, NOTES_PROJECT, note, null));

describe("mock notes", () => {
	it("publishes the sample notes on both devices", () => {
		const { notes } = setup();
		const entities = notes.entities() as ({ entity: "project_notes" } & ProjectNotes)[];
		expect(entities.map((e) => e.device)).toEqual([MAC, DEVBOX]);
		const titles = entities[0]!.notes.map((n) => n.title);
		expect(titles).toEqual(["GTFS release steps", "Design notes"]);
		expect(entities[0]!.notes[0]!.created_on).toBe(DEVBOX);
		expect(entities[1]!.notes).toEqual(entities[0]!.notes);
	});

	it("holds a checklist and a design note with a table and a diagram", async () => {
		const { notes } = setup();
		const checklist = await head(notes);
		expect(checklist.body).toContain("- [x] ");
		expect(checklist.body).toContain("- [ ] ");
		const design = await head(notes, DESIGN);
		expect(design.body).toContain("| Stage |");
		expect(design.body).toContain("```mermaid");
	});

	it("saves from the head without merging", async () => {
		const { notes, changed } = setup();
		const before = await head(notes);
		const saved = ok(
			await notes.calls.saveNote(DEVBOX, NOTES_PROJECT, CHECKLIST, {
				base: before.hash,
				title: before.title,
				body: before.body.replace("- [ ] Tag", "- [x] Tag"),
				deleted: false,
			})
		);
		expect(saved.merged).toBe(false);
		expect(saved.version.parents).toEqual([before.hash]);
		expect(saved.version.by).toEqual({ device: DEVBOX, agent: null });
		expect(changed).toHaveBeenCalledOnce();
		const summary = (notes.entities()[0] as ProjectNotes).notes[0]!;
		expect(summary.head).toBe(saved.version.hash);
	});

	it("merges a save from an old version with the head", async () => {
		const { notes } = setup();
		const base = await head(notes);
		await notes.calls.saveNote(MAC, NOTES_PROJECT, CHECKLIST, {
			base: base.hash,
			title: base.title,
			body: base.body.replace("- [ ] Tag", "- [x] Tag"),
			deleted: false,
		});
		const saved = ok(
			await notes.calls.saveNote(DEVBOX, NOTES_PROJECT, CHECKLIST, {
				base: base.hash,
				title: base.title,
				body: base.body.replace("- [ ] Post", "- [x] Post"),
				deleted: false,
			})
		);
		expect(saved.merged).toBe(true);
		expect(saved.version.body).toContain("- [x] Tag");
		expect(saved.version.body).toContain("- [x] Post");
		expect(saved.version.parents).toHaveLength(2);
		const history = ok(await notes.calls.noteHistory(MAC, NOTES_PROJECT, CHECKLIST));
		expect(history).toHaveLength(5);
		expect(history[0]!.hash).toBe(saved.version.hash);
	});

	it("creates a note from no base and reads an old version", async () => {
		const { notes } = setup();
		const made = ok(
			await notes.calls.saveNote(MAC, NOTES_PROJECT, "note-new", {
				base: null,
				title: "Ideas",
				body: "alpha\n",
				deleted: false,
			})
		);
		expect(made.version.parents).toEqual([]);
		const history = ok(await notes.calls.noteHistory(MAC, NOTES_PROJECT, CHECKLIST));
		const first = history[history.length - 1]!;
		const old = ok(await notes.calls.readNote(DEVBOX, NOTES_PROJECT, CHECKLIST, first.hash));
		expect(old.body).toContain("- [ ] Refresh");
	});

	it("refuses what maxd refuses", async () => {
		const { notes } = setup();
		const save = {
			base: null,
			title: "x".repeat(MAX_NOTE_TITLE + 1),
			body: "",
			deleted: false,
		};
		expect((await notes.calls.saveNote(MAC, NOTES_PROJECT, "note-new", save)).status).toBe(
			"error"
		);
		const stale = { base: "f".repeat(64), title: "Plan", body: "", deleted: false };
		expect((await notes.calls.saveNote(MAC, NOTES_PROJECT, CHECKLIST, stale)).status).toBe(
			"error"
		);
		expect(
			(await notes.calls.readNote("dev-other", NOTES_PROJECT, CHECKLIST, null)).status
		).toBe("error");
		expect((await notes.calls.readNote(MAC, NOTES_PROJECT, "note-none", null)).status).toBe(
			"error"
		);
	});
});
