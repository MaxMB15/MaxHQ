// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A project's notes as the sidebar lists them, and the device a client
// reads and writes them through. Each note shows at its newest report, by
// `./note-heads`. The desktop goes through the Mac's own `maxd`, which is
// always there, and while it isn't connected it shows a note read-only
// from another device that holds it. The phone has no `maxd` of its own,
// so it writes through a connected device that holds the project's notes
// or has a workspace in it, and the devices sync the change between them.
// It also holds what a note's pill needs: the notes the `@` menu offers,
// the first lines its preview shows, the name of the copy a note from
// another project goes with, and whether an agent's `maxd` takes the pill.

import { t } from "../i18n";

import {
	type DeviceId,
	NOTE_PILLS_SINCE,
	type NoteAuthor,
	type NoteId,
	type NoteSummary,
	type ProjectId,
} from "../bindings";
import type { NoteRef } from "../pills/draft";
import { noteLabel } from "../pills/label";
import { newestNotes, projectsWithNotes, reportsOf } from "./note-heads";
import { all, type ViewState } from "./replica";
import { deviceName, linkForDevice } from "./select";

export { projectsWithNotes } from "./note-heads";

/** One live note in a project, as the sidebar lists it. */
export type NoteRow = NoteSummary & {
	/** The title to show, which tells notes with the same title apart. */
	readonly shown: string;
	/** The device whose report this is. */
	readonly from: DeviceId;
};

/** How many characters of a note's ID tell it apart from another with the same title. */
const SHORT_ID = 8;

const natural = (a: string, b: string): number =>
	a.localeCompare(b, undefined, { numeric: true, sensitivity: "base" });

/**
 * A note as the newest report has it, deleted or not, or undefined when
 * no device reports it.
 */
export const noteSummary = (
	view: ViewState,
	project: ProjectId,
	note: NoteId
): NoteSummary | undefined => newestNotes(reportsOf(view, project)).get(note)?.summary;

/** A note as one device reports it, or undefined when that device doesn't. */
export const noteOn = (
	view: ViewState,
	device: DeviceId,
	project: ProjectId,
	note: NoteId
): NoteSummary | undefined =>
	reportsOf(view, project)
		.find((list) => list.device === device)
		?.notes.find((summary) => summary.note === note);

/**
 * A project's live notes, each at its newest report, sorted by title in
 * natural order. Two notes with the same title read "Title (device)", after
 * the device each was made on, or "Title (id)" when that's the same device.
 */
export const notesOf = (view: ViewState, project: ProjectId): NoteRow[] => {
	const live = [...newestNotes(reportsOf(view, project)).values()].filter(
		({ summary }) => !summary.deleted
	);
	const byTitle = new Map<string, NoteSummary[]>();
	for (const { summary } of live) {
		byTitle.set(summary.title, [...(byTitle.get(summary.title) ?? []), summary]);
	}
	const shownOf = (note: NoteSummary): string => {
		const same = byTitle.get(note.title) ?? [];
		if (same.length < 2) return note.title;
		const sharesDevice = same.some(
			(other) => other.note !== note.note && other.created_on === note.created_on
		);
		const tag = sharesDevice ? note.note.slice(0, SHORT_ID) : deviceName(view, note.created_on);
		return `${note.title} (${tag})`;
	};
	return live
		.map(({ summary, from }) => ({ ...summary, shown: shownOf(summary), from }))
		.sort((a, b) => natural(a.shown, b.shown) || natural(a.note, b.note));
};

/**
 * The first free title of "Untitled", "Untitled 2", "Untitled 3" and so on
 * among a project's live notes.
 */
export const untitled = (view: ViewState, project: ProjectId): string => {
	const taken = new Set(notesOf(view, project).map((row) => row.title));
	let title = t("Untitled");
	for (let n = 2; taken.has(title); n++) title = t("Untitled {{n}}", { n: n });
	return title;
};

/** Where a client reads and writes a project's notes. */
export type NotesReach =
	/** Through this device. `readOnly` is set when it isn't this Mac, with why in `why`. */
	| { readonly device: DeviceId; readonly readOnly: boolean; readonly why: string | null }
	/** Nowhere, since no device that holds them is connected. */
	| { readonly device: null; readonly readOnly: true; readonly why: string };

const connected = (view: ViewState, device: DeviceId): boolean =>
	linkForDevice(view, device)?.state.state === "connected";

/**
 * The device a client reads and writes a project's notes through: this
 * Mac's own `maxd` while it's connected, since it keeps a copy of every
 * project's notes. Else the first connected device that reports the
 * project's notes, read-only, since a save there wouldn't reach the Mac
 * until it's back.
 */
export const notesReach = (view: ViewState, project: ProjectId): NotesReach => {
	const links = all(view, "device_link");
	const here = links.find((l) => l.route.type === "local")?.device ?? null;
	if (here && connected(view, here)) return { device: here, readOnly: false, why: null };
	const holders = new Set(reportsOf(view, project).map((list) => list.device));
	const other = links.find(
		(l) =>
			l.device !== null &&
			l.device !== here &&
			holders.has(l.device) &&
			connected(view, l.device)
	)?.device;
	const mac = here
		? t("This Mac's maxd isn't connected")
		: t("This client has no maxd of its own");
	if (other)
		return {
			device: other,
			readOnly: true,
			why: t("{{mac}}, so this is {{deviceName_view__other}}'s copy, read-only.", {
				mac: mac,
				deviceName_view__other: deviceName(view, other),
			}),
		};
	return {
		device: null,
		readOnly: true,
		why: t("{{mac}}, and no device that has these notes is.", { mac: mac }),
	};
};

/**
 * The device a client with no `maxd` of its own, such as the phone, reads
 * and writes a project's notes through: the first connected device that
 * reports the project's notes, else the first connected one with a
 * workspace in the project, which takes notes for it. A save there
 * reaches the other devices when they sync.
 */
export const remoteNotesReach = (view: ViewState, project: ProjectId): NotesReach => {
	const up = all(view, "device_link").flatMap((l) =>
		l.device !== null && connected(view, l.device) ? [l.device] : []
	);
	const holders = new Set(reportsOf(view, project).map((list) => list.device));
	const homes = new Set(
		all(view, "workspace").flatMap((w) => (w.project === project ? [w.device] : []))
	);
	const device = up.find((d) => holders.has(d)) ?? up.find((d) => homes.has(d));
	if (device !== undefined) return { device, readOnly: false, why: null };
	return {
		device: null,
		readOnly: true,
		why: t("No device that has this project is connected."),
	};
};

/**
 * A project's Notes entry, or null when it shows none: it shows while the
 * project has a live note, or while `reach` can write a new one.
 */
export const notesEntry = (
	view: ViewState,
	project: ProjectId,
	reach: NotesReach
): { readonly count: number; readonly writable: boolean } | null => {
	const count = notesOf(view, project).length;
	const writable = !reach.readOnly;
	return count > 0 || writable ? { count, writable } : null;
};

/** Who saved a note version: you or an agent on a device, or "Merged" for a merge. */
export const savedBy = (view: ViewState, by: NoteAuthor | null): string => {
	if (!by) return t("Merged");
	const device = deviceName(view, by.device);
	if (by.agent === null) return device;
	const agent = view.tables.agent.get(by.agent)?.title ?? t("An agent");
	return t("{{value1}} on {{value2}}", { value1: agent, value2: device });
};

/** A note's title to show, or "Untitled" when it has none. */
export const shownTitle = noteLabel;

/**
 * Why a note can't go as a pill to an agent on `device`, or null when it
 * can. An older `maxd` doesn't know note pills, and one whose version isn't
 * known yet is taken as older.
 */
export const notePillBlocked = (view: ViewState, device: DeviceId): string | null => {
	const protocol = linkForDevice(view, device)?.protocol ?? null;
	if (protocol !== null && protocol >= NOTE_PILLS_SINCE) return null;
	return t(
		"maxd on {{deviceName_view__device}} is older than this app; update it to send notes",
		{ deviceName_view__device: deviceName(view, device) }
	);
};

/** How many lines of a note its pill's preview shows. */
export const PREVIEW_LINES = 4;

/**
 * A note's first lines with text, for its pill's preview, each with the
 * spaces at its ends taken off.
 */
export const firstLines = (body: string, count = PREVIEW_LINES): string[] =>
	body
		.split(/\r?\n/)
		.map((line) => line.trim())
		.filter((line) => line !== "")
		.slice(0, count);

/** The longest a note copy's name gets before `.md`, in characters. */
const COPY_NAME_MAX = 60;

/**
 * The file name a note's read-only copy goes as, for an agent in another
 * project: its title with the characters a file name can't hold taken
 * out, then `.md`.
 */
export const noteCopyName = (title: string): string => {
	const clean = Array.from(
		noteLabel(title)
			.replace(/[\p{Cc}/\\:*?"<>|]/gu, " ")
			.replace(/\s+/g, " ")
			.trim()
			.replace(/^\.+/, "")
	)
		.slice(0, COPY_NAME_MAX)
		.join("")
		.trim();
	return `${clean === "" ? t("Note") : clean}.md`;
};

/** A note the `@` menu offers, with the project it's in. */
export type NoteChoice = {
	readonly ref: NoteRef;
	/** The title to show, which tells notes with the same title apart. */
	readonly shown: string;
	readonly projectName: string;
	/** Whether it's in the agent's own project. */
	readonly own: boolean;
};

/**
 * The live notes whose title holds `query`, for the `@` menu: those in the
 * agent's own project first, then the other projects' by project name,
 * each project's in the sidebar's order.
 */
export const noteChoices = (
	view: ViewState,
	own: ProjectId | null,
	query: string
): NoteChoice[] => {
	const wanted = query.toLowerCase();
	const nameOf = (project: ProjectId) => view.tables.project.get(project)?.name ?? t("Project");
	const projects = [...projectsWithNotes(view)].sort(
		(a, b) => Number(b === own) - Number(a === own) || natural(nameOf(a), nameOf(b))
	);
	return projects.flatMap((project) =>
		notesOf(view, project)
			.filter((row) => row.shown.toLowerCase().includes(wanted))
			.map((row) => ({
				ref: { type: "note", project, note: row.note, title: row.title, copy: null },
				shown: noteLabel(row.shown),
				projectName: nameOf(project),
				own: project === own,
			}))
	);
};
