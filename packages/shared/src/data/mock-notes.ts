// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The project note calls of the sample data. The AtlasMaps project, which
// the GTFS agent works in, holds a release checklist the agent started and
// design notes written on the Mac. Every sample device holds every
// version, as if they'd just synced, and each publishes its
// `project_notes` entity the way `maxd` does. A save from an old version
// merges with the head, by the rules in `../notes/merge`.

import {
	MAX_NOTE_BODY,
	MAX_NOTE_TITLE,
	type CommandError,
	type DeviceId,
	type Entity,
	type NoteAuthor,
	type NoteId,
	type NoteSummary,
	type NoteVersion,
	type ProjectId,
	type Timestamp,
} from "../bindings";
import { heads, sealed, settle, utf8 } from "../notes/merge";
import { DEVBOX, MAC } from "./mock-view";
import type { DataSource } from "./source";

/** The project note calls of a data source. */
export type NoteCalls = Pick<DataSource, "readNote" | "saveNote" | "noteHistory">;

/** The sample notes, and the entities that carry them. */
export type MockNotes = {
	readonly calls: NoteCalls;
	/** Each device's `project_notes` entity for each project with notes. */
	readonly entities: () => Entity[];
	/** Writes a new note in the sample project, as an agent's `create_note` does. */
	readonly write: (note: NoteId, title: string, body: string, by: NoteAuthor) => void;
};

const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

/** The devices that hold the sample notes. */
const DEVICES: readonly DeviceId[] = [MAC, DEVBOX];

/** The project the sample notes are in. */
export const NOTES_PROJECT: ProjectId = "proj-atlas-maps";

/** The release checklist, which the GTFS agent started on devbox. */
export const CHECKLIST: NoteId = "note-release-checklist";
/** The design notes, written on the Mac. */
export const DESIGN: NoteId = "note-design";

/** The most versions a history answers with, as `MAX_NOTE_HISTORY` in Rust. */
const HISTORY = 200;

const CHECKLIST_BODY = `## Before the release

- [x] Refresh the GTFS feed
- [x] Rebuild the tile cache
- [ ] Check stop names on the night routes
- [ ] Tag v2.4.0
- [ ] Post the changelog
`;

const DESIGN_BODY = `# Design notes

How a feed refresh reaches the map.

| Stage | Runs on | Takes |
| --- | --- | --- |
| Fetch | devbox | 2 min |
| Validate | devbox | 30 s |
| Tiles | ws-alpha | 6 min |

\`\`\`mermaid
flowchart LR
  feed[GTFS feed] --> check[Validate]
  check --> tiles[Build tiles]
  tiles --> map[Map]
\`\`\`
`;

const failed = (message: string): { status: "error"; error: CommandError } => ({
	status: "error",
	error: { kind: "failed", message },
});

const notFound = (what: string): { status: "error"; error: CommandError } => ({
	status: "error",
	error: { kind: "not_found", what },
});

/** A body's size in bytes, as `maxd` counts it. */
const size = (body: string): number => utf8(body).length;

/** Refuses a title or body over the limits, with `maxd`'s words. */
const overLimits = (title: string, body: string): string | null => {
	if ([...title].length > MAX_NOTE_TITLE)
		return `A note's title can be at most ${MAX_NOTE_TITLE} characters.`;
	if (size(body) > MAX_NOTE_BODY)
		return `A note can be at most ${MAX_NOTE_BODY / (1024 * 1024)} MiB.`;
	return null;
};

/**
 * The sample notes, kept in memory. `changed` runs after each save, so the
 * caller can publish the new entities.
 */
export const mockNotes = ({
	now,
	changed,
}: {
	now: () => Timestamp;
	changed: () => void;
}): MockNotes => {
	const start = now();
	const versions = new Map<ProjectId, NoteVersion[]>();
	const createdOn = new Map<NoteId, DeviceId>();

	const add = (version: NoteVersion) => {
		const held = versions.get(version.project) ?? [];
		if (!held.some((v) => v.hash === version.hash)) held.push(version);
		versions.set(version.project, held);
	};
	const ofNote = (project: ProjectId, note: NoteId): NoteVersion[] =>
		(versions.get(project) ?? []).filter((v) => v.note === note);
	const headOf = (project: ProjectId, note: NoteId): NoteVersion | undefined =>
		heads(ofNote(project, note))[0];

	/** Seals and keeps a sample version. */
	const sample = (
		note: NoteId,
		parent: NoteVersion | null,
		title: string,
		body: string,
		at: Timestamp,
		by: NoteAuthor | null
	): NoteVersion => {
		const version = sealed({
			hash: "",
			project: NOTES_PROJECT,
			note,
			parents: parent ? [parent.hash] : [],
			title,
			body,
			deleted: false,
			at,
			by,
		});
		add(version);
		return version;
	};

	const gtfs: NoteAuthor = { device: DEVBOX, agent: "agent-gtfs" };
	const you: NoteAuthor = { device: MAC, agent: null };
	createdOn.set(CHECKLIST, DEVBOX);
	const checklist = sample(
		CHECKLIST,
		null,
		"GTFS release steps",
		CHECKLIST_BODY.replace("- [x] Rebuild", "- [ ] Rebuild").replace(
			"- [x] Refresh",
			"- [ ] Refresh"
		),
		start - 2 * DAY,
		gtfs
	);
	sample(CHECKLIST, checklist, "GTFS release steps", CHECKLIST_BODY, start - 40 * MIN, gtfs);
	createdOn.set(DESIGN, MAC);
	const design = sample(
		DESIGN,
		null,
		"Design notes",
		DESIGN_BODY.slice(0, DESIGN_BODY.indexOf("```")),
		start - 3 * DAY,
		you
	);
	sample(DESIGN, design, "Design notes", DESIGN_BODY, start - 5 * HOUR, you);

	const summary = (project: ProjectId, note: NoteId): NoteSummary | null => {
		const head = headOf(project, note);
		if (!head) return null;
		return {
			note,
			title: head.title,
			head: head.hash,
			at: head.at,
			by: head.by,
			created_on: createdOn.get(note) ?? MAC,
			deleted: head.deleted,
			size: size(head.body),
		};
	};

	const calls: NoteCalls = {
		readNote: async (device, project, note, version) => {
			if (!DEVICES.includes(device)) return notFound(`device ${device}`);
			const found = version
				? ofNote(project, note).find((v) => v.hash === version)
				: headOf(project, note);
			if (!found) return notFound(version ? `note version ${version}` : `note ${note}`);
			return { status: "ok", data: found };
		},
		saveNote: async (device, project, note, save) => {
			if (!DEVICES.includes(device)) return notFound(`device ${device}`);
			const over = overLimits(save.title, save.body);
			if (over) return failed(over);
			if (save.base !== null && !ofNote(project, note).some((v) => v.hash === save.base)) {
				return failed("That note version isn't on this device.");
			}
			if (!createdOn.has(note)) createdOn.set(note, device);
			add(
				sealed({
					hash: "",
					project,
					note,
					parents: save.base === null ? [] : [save.base],
					title: save.title,
					body: save.body,
					deleted: save.deleted,
					at: now(),
					by: { device, agent: null },
				})
			);
			const made = settle(ofNote(project, note));
			for (const version of made) add(version);
			const head = headOf(project, note);
			if (!head) return failed("The note has no head.");
			changed();
			return { status: "ok", data: { version: head, merged: made.length > 0 } };
		},
		noteHistory: async (device, project, note) => {
			if (!DEVICES.includes(device)) return notFound(`device ${device}`);
			const all = ofNote(project, note);
			if (all.length === 0) return notFound(`note ${note}`);
			// Newest first, and of two saved at once, the one kept later, so a
			// merge comes before the heads it joins.
			const newest = all
				.map((v, kept) => ({ v, kept }))
				.sort((p, q) => q.v.at - p.v.at || q.kept - p.kept)
				.slice(0, HISTORY)
				.map(({ v }) => v);
			return {
				status: "ok",
				data: newest.map((v) => ({
					hash: v.hash,
					parents: v.parents,
					title: v.title,
					deleted: v.deleted,
					at: v.at,
					by: v.by,
					size: size(v.body),
				})),
			};
		},
	};

	return {
		calls,
		entities: () =>
			[...versions.keys()].flatMap((project) => {
				const notes = [...new Set((versions.get(project) ?? []).map((v) => v.note))]
					.map((note) => summary(project, note))
					.filter((s): s is NoteSummary => s !== null);
				return DEVICES.map((device) => ({
					entity: "project_notes" as const,
					device,
					project,
					notes,
				}));
			}),
		write: (note, title, body, by) => {
			createdOn.set(note, by.device);
			sample(note, null, title, body, now(), by);
			changed();
		},
	};
};
