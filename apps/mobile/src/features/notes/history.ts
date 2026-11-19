// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A note's history on the phone, as on the desktop: its versions newest
// first, and an old one read-only with a way back and Restore. The note
// screen shows the note, its history or one old version, and these read
// what the last two show.

import type {
	CommandError,
	DeviceId,
	NoteId,
	NoteVersion,
	NoteVersionInfo,
	ProjectId,
	VersionHash,
} from "@maxhq/shared/bindings";
import type { DataSource } from "@maxhq/shared/data/source";
import { errorText } from "@maxhq/shared/timeline/store";

/** What the note screen shows. */
export type Showing =
	| { readonly kind: "note" }
	| { readonly kind: "history" }
	| { readonly kind: "version"; readonly hash: VersionHash };

/** A read from the device: on its way, failed with why, or done. */
export type Loaded<T> =
	| { readonly kind: "reading" }
	| { readonly kind: "failed"; readonly reason: string }
	| { readonly kind: "read"; readonly data: T };

/** A device's answer as what the screen shows. */
export const loaded = <T>(
	result: { status: "ok"; data: T } | { status: "error"; error: CommandError }
): Loaded<T> =>
	result.status === "ok"
		? { kind: "read", data: result.data }
		: { kind: "failed", reason: errorText(result.error) };

// A read that threw, rather than answered, still says why.
const thrown = (error: unknown): Loaded<never> => ({
	kind: "failed",
	reason: error instanceof Error ? error.message : String(error),
});

/** A note's versions on `device`, newest first. */
export const readHistory = (
	source: Pick<DataSource, "noteHistory">,
	device: DeviceId,
	project: ProjectId,
	note: NoteId
): Promise<Loaded<NoteVersionInfo[]>> =>
	source.noteHistory(device, project, note).then(loaded, thrown);

/** One version of a note on `device`, with its text. */
export const readVersion = (
	source: Pick<DataSource, "readNote">,
	device: DeviceId,
	project: ProjectId,
	note: NoteId,
	hash: VersionHash
): Promise<Loaded<NoteVersion>> =>
	source.readNote(device, project, note, hash).then(loaded, thrown);

/**
 * What picking a row of the history shows: the newest is the note as it
 * is, so it goes back to it, and any other opens read-only.
 */
export const pickVersion = (index: number, hash: VersionHash): Showing =>
	index === 0 ? { kind: "note" } : { kind: "version", hash };

/** Where Back goes from what shows: an old version goes back to the history, the history to the note. */
export const stepBack = (showing: Showing): Showing =>
	showing.kind === "version" ? { kind: "history" } : { kind: "note" };
