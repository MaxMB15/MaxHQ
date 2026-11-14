// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The project note reads and changes, for the pane and the sidebar: a
// note's head or an old version, its history, and making, renaming,
// deleting and bringing back a note. A change to a note open in a pane goes
// through that pane's saving, so the two never race; one that isn't open
// reads the head and saves on it. A read the device refuses fails with the
// reason in words.

import { useQuery } from "@tanstack/react-query";

import { type Services, useServices } from "@/app/services";
import type {
	CommandError,
	DeviceId,
	NoteId,
	ProjectId,
	VersionHash,
} from "@maxhq/shared/bindings";
import { changeHead, makeNote } from "@maxhq/shared/notes/actions";
import { errorText } from "@maxhq/shared/timeline/store";
import { notesReach } from "@maxhq/shared/view/notes";

import { openNote, openSaving } from "./open";

const NOTES = "notes";

/** A result's data, or a throw with the device's reason in words. */
const dataOf = <T>(
	result: { status: "ok"; data: T } | { status: "error"; error: CommandError }
): T => {
	if (result.status === "error") throw new Error(errorText(result.error));
	return result.data;
};

/**
 * A note's head on `device`, or one of its versions. A head is read once
 * per opening, since the pane follows it after; a version never changes.
 */
export const useNoteVersion = (
	device: DeviceId | null,
	project: ProjectId,
	note: NoteId,
	version: VersionHash | null = null
) => {
	const { source } = useServices();
	return useQuery({
		queryKey: [NOTES, "version", device, project, note, version],
		queryFn: async () => dataOf(await source.readNote(device!, project, note, version)),
		enabled: device !== null,
		staleTime: Infinity,
		refetchOnWindowFocus: false,
		// The next opening reads the head again.
		gcTime: version === null ? 0 : undefined,
	});
};

/** A note's versions on `device`, newest first, read again when its head moves. */
export const useNoteHistory = (
	device: DeviceId | null,
	project: ProjectId,
	note: NoteId,
	head: VersionHash | null,
	enabled: boolean
) => {
	const { source } = useServices();
	return useQuery({
		queryKey: [NOTES, "history", device, project, note, head],
		queryFn: async () => dataOf(await source.noteHistory(device!, project, note)),
		enabled: enabled && device !== null,
		placeholderData: (previous) => previous,
	});
};

/**
 * Makes an empty note titled "Untitled", or the next free "Untitled 2" and
 * so on, and opens it with the title selected. Resolves with why it
 * couldn't, or null.
 */
export const createNote = async (
	services: Services,
	project: ProjectId
): Promise<string | null> => {
	const { view } = services.view.getState();
	const made = await makeNote(services.source, view, notesReach(view, project), project);
	if ("failure" in made) return made.failure;
	openNote(services, project, made.note, { made: true });
	return null;
};

/**
 * Renames a note, deletes it or brings it back, as a new version on its
 * head. Resolves with why it couldn't, or null.
 */
export const changeNote = async (
	services: Services,
	project: ProjectId,
	note: NoteId,
	change: { readonly title?: string; readonly deleted?: boolean }
): Promise<string | null> => {
	const saving = openSaving(project, note);
	if (saving) return saving.replace({ title: change.title, deleted: change.deleted });
	const reach = notesReach(services.view.getState().view, project);
	return changeHead(services.source, reach, project, note, change);
};
