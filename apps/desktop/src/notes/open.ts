// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The notes open in this window's panes. Each open note's saving is kept
// here by project and note, so its tab shows the title being typed and an
// unsaved dot, and a rename or delete from the sidebar goes through the
// same saving as the pane. What waits is saved when the window goes.

import { useCallback, useSyncExternalStore } from "react";

import type { Services } from "@/app/services";
import { useServices } from "@/app/services";
import { type OpenOptions, panesOf } from "@/layout/panes";
import type { NoteId, ProjectId } from "@maxhq/shared/bindings";

import type { Autosave, NoteSnapshot } from "@maxhq/shared/notes/autosave";

import type { NoteEditorHandle } from "./editor";

const keyOf = (project: ProjectId, note: NoteId) => `${project}/${note}`;

const open = new Map<string, Autosave>();
const listeners = new Set<() => void>();
const changed = () => {
	for (const listener of [...listeners]) listener();
};

let unloading = false;
const saveAll = () => {
	for (const saving of open.values()) void saving.flush();
};

const editors = new Map<string, () => NoteEditorHandle | null>();

/**
 * Keeps an open note's saving here until the returned call, with its
 * editor, through which Add to note puts a visual at the caret.
 */
export const keepOpen = (
	project: ProjectId,
	note: NoteId,
	saving: Autosave,
	editor?: () => NoteEditorHandle | null
): (() => void) => {
	if (!unloading) {
		unloading = true;
		window.addEventListener("pagehide", saveAll);
		window.addEventListener("beforeunload", saveAll);
	}
	const key = keyOf(project, note);
	open.set(key, saving);
	if (editor) editors.set(key, editor);
	changed();
	return () => {
		if (open.get(key) === saving) {
			open.delete(key);
			editors.delete(key);
		}
		changed();
	};
};

/** The saving of a note open in a pane, if it is. */
export const openSaving = (project: ProjectId, note: NoteId): Autosave | undefined =>
	open.get(keyOf(project, note));

/** The editor of a note open in a pane, if it is and has one. */
export const openEditor = (project: ProjectId, note: NoteId): NoteEditorHandle | null =>
	editors.get(keyOf(project, note))?.() ?? null;

const subscribeOpen = (listener: () => void) => {
	listeners.add(listener);
	return () => {
		listeners.delete(listener);
	};
};

/** What an open note's pane shows of it, or null while it isn't open. */
export const useOpenNote = (project: ProjectId, note: NoteId): NoteSnapshot | null => {
	const subscribe = useCallback(
		(notify: () => void) => {
			let stop: (() => void) | undefined;
			const follow = () => {
				stop?.();
				stop = openSaving(project, note)?.subscribe(notify);
				notify();
			};
			const unlisten = subscribeOpen(follow);
			follow();
			return () => {
				unlisten();
				stop?.();
			};
		},
		[project, note]
	);
	return useSyncExternalStore(subscribe, () => openSaving(project, note)?.snapshot() ?? null);
};

// Notes just made, whose pane opens with the title selected.
const fresh = new Set<string>();

/** Whether the note was just made, once: the next ask says no. */
export const takeFresh = (project: ProjectId, note: NoteId): boolean =>
	fresh.delete(keyOf(project, note));

/**
 * Opens a note's pane or focuses it, in projects mode with no PR page over
 * the panes. A note just made opens with its title selected.
 */
export const openNote = (
	services: Services,
	project: ProjectId,
	note: NoteId,
	{ made = false, ...options }: OpenOptions & { readonly made?: boolean } = {}
): void => {
	if (made) fresh.add(keyOf(project, note));
	panesOf(services).openNote(project, note, options);
	services.ui.setState({ mode: "projects", pullsOf: null });
};

/** The ID of the pane with focus in this window, or null. */
export const useActivePane = (): string | null => {
	const panes = panesOf(useServices());
	const subscribe = useCallback(
		(notify: () => void) => {
			let watching: { dispose: () => void } | undefined;
			const unwatch = panes.watch((api) => {
				watching?.dispose();
				watching = api?.onDidActivePanelChange(notify);
				notify();
			});
			return () => {
				unwatch();
				watching?.dispose();
			};
		},
		[panes]
	);
	return useSyncExternalStore(subscribe, () => panes.api()?.activePanel?.id ?? null);
};
