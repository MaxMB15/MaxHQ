// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Making a note, and renaming, deleting or bringing back one that isn't
// open, for any client. Each goes to the device a client reaches the
// project's notes through, from `../view/notes`. A note open on screen
// changes through its own saving in `./autosave` instead, so the two
// never race. A change the device refuses resolves with the reason in
// words.

import { t } from "../i18n";

import type { DeviceId, NoteId, ProjectId } from "../bindings";
import type { DataSource } from "../data/source";
import { errorText } from "../timeline/store";
import { type NotesReach, untitled } from "../view/notes";
import type { ViewState } from "../view/replica";

/** The device a change to a project's notes goes to, or why there's none. */
export const writerOf = (
	reach: NotesReach
):
	| { readonly device: DeviceId; readonly why: null }
	| { readonly device: null; readonly why: string } =>
	reach.device !== null && !reach.readOnly
		? { device: reach.device, why: null }
		: { device: null, why: reach.why ?? t("These notes are read-only here.") };

/** What to ask before a note is deleted. */
export const deleteNoteQuestion = (
	title: string
): { readonly title: string; readonly detail: string } => ({
	title: t('Delete note "{{title}}"?', { title: title }),
	detail: t("It goes from every device. Its history stays, so it can be brought back."),
});

/**
 * A new note's ID, a random version 4 UUID. React Native may have no
 * `crypto.randomUUID`, so it falls back to `crypto.getRandomValues`, and
 * to `Math.random` where neither is there.
 */
export const newNoteId = (): NoteId => {
	const crypto = (
		globalThis as {
			crypto?: {
				randomUUID?: () => string;
				getRandomValues?: (bytes: Uint8Array) => Uint8Array;
			};
		}
	).crypto;
	if (typeof crypto?.randomUUID === "function") return crypto.randomUUID();
	const bytes = new Uint8Array(16);
	if (typeof crypto?.getRandomValues === "function") crypto.getRandomValues(bytes);
	else for (let i = 0; i < bytes.length; i++) bytes[i] = Math.floor(Math.random() * 256);
	bytes[6] = ((bytes[6] ?? 0) & 0x0f) | 0x40;
	bytes[8] = ((bytes[8] ?? 0) & 0x3f) | 0x80;
	const hex = [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
	return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
};

/**
 * Makes a note through `reach`, empty or holding `body`, titled "Untitled"
 * or the next free "Untitled 2" and so on. Resolves with its ID, or why it
 * couldn't.
 */
export const makeNote = async (
	source: DataSource,
	view: ViewState,
	reach: NotesReach,
	project: ProjectId,
	body = ""
): Promise<{ readonly note: NoteId } | { readonly failure: string }> => {
	const { device, why } = writerOf(reach);
	if (device === null) return { failure: why };
	const note = newNoteId();
	const result = await source.saveNote(device, project, note, {
		base: null,
		title: untitled(view, project),
		body,
		deleted: false,
	});
	return result.status === "error" ? { failure: errorText(result.error) } : { note };
};

/** A change to a note's text, from its head's: the new text, or why it can't. */
export type BodyChange = (body: string) => string | { readonly refused: string };

/**
 * Renames a note through `reach`, changes its text, deletes it or brings it
 * back, as a new version on its head. Resolves with why it couldn't, or
 * null.
 */
export const changeHead = async (
	source: DataSource,
	reach: NotesReach,
	project: ProjectId,
	note: NoteId,
	change: {
		readonly title?: string;
		readonly body?: BodyChange;
		readonly deleted?: boolean;
	}
): Promise<string | null> => {
	const { device, why } = writerOf(reach);
	if (device === null) return why;
	const head = await source.readNote(device, project, note, null);
	if (head.status === "error") return errorText(head.error);
	const body = change.body ? change.body(head.data.body) : head.data.body;
	if (typeof body !== "string") return body.refused;
	const result = await source.saveNote(device, project, note, {
		base: head.data.hash,
		title: change.title ?? head.data.title,
		body,
		deleted: change.deleted ?? head.data.deleted,
	});
	return result.status === "error" ? errorText(result.error) : null;
};
