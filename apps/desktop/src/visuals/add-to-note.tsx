// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Add to note, from a visual's card, and a visual dropped on a note in the
// sidebar. The card lists the notes of its project: the project of the
// agent whose reply holds it, or of the note it's in. A note open in a pane
// takes the visual at its caret, as an edit there; one that isn't gets it
// at its end, as a new version on its head. New note makes one holding
// only the visual. Each shows a toast with Open and Undo, and Undo takes
// out exactly that block, or says why it can't.

import { t } from "@maxhq/shared/i18n";

import { NotebookPen, NotebookText, Plus } from "lucide-react";
import { createContext, use, useMemo } from "react";

import { type Services, useServicesIfAny } from "@/app/services";
import { openEditor, openNote } from "@/notes/open";
import type { AgentId, NoteId, ProjectId } from "@maxhq/shared/bindings";
import { changeHead, makeNote, writerOf } from "@maxhq/shared/notes/actions";
import { appendVisual, type Placed, projectOfAgent, takeOut } from "@maxhq/shared/notes/visuals";
import { noteSummary, notesOf, notesReach, untitled } from "@maxhq/shared/view/notes";
import type { Visual } from "@maxhq/shared/visuals/fence";

import type { VisualAction } from "./actions";
import { showAdded } from "./added-toast";
import { useVisualOrigin } from "./origin";

/** The note a visual's card is drawn in. */
export const VisualNoteContext = createContext<{
	readonly project: ProjectId;
	readonly note: NoteId;
} | null>(null);

const changed = (): string => t("The visual changed since it went in, so it stays.");

/** The title the sidebar shows for a note. */
const titleOf = (services: Services, project: ProjectId, note: NoteId): string => {
	const { view } = services.view.getState();
	return (
		notesOf(view, project).find((row) => row.note === note)?.shown ??
		noteSummary(view, project, note)?.title ??
		t("the note")
	);
};

/** Takes a placed block out of a note, wherever it is now. */
const undoPlaced = async (
	services: Services,
	project: ProjectId,
	note: NoteId,
	placed: Placed
): Promise<string | null> => {
	const editor = openEditor(project, note);
	if (editor) return editor.removePlaced(placed) ? null : changed();
	const reach = notesReach(services.view.getState().view, project);
	return changeHead(services.source, reach, project, note, {
		body: (body) => takeOut(body, placed) ?? { refused: changed() },
	});
};

/**
 * Puts a visual in a note: at the caret when it's open, at its end when it
 * isn't. Shows the toast, or why it couldn't.
 */
export const addVisualToNote = async (
	services: Services,
	project: ProjectId,
	note: NoteId,
	visual: Visual
): Promise<void> => {
	const put: { placed: Placed | null } = { placed: null };
	const editor = openEditor(project, note);
	let why: string | null;
	if (editor) {
		const placed = editor.insertVisual(visual);
		why = "refused" in placed ? placed.refused : null;
		if (!("refused" in placed)) put.placed = placed;
	} else {
		const reach = notesReach(services.view.getState().view, project);
		why = await changeHead(services.source, reach, project, note, {
			body: (body) => {
				const placing = appendVisual(body, visual);
				if ("refused" in placing) return placing;
				put.placed = placing.placed;
				return placing.body;
			},
		});
	}
	const done = put.placed;
	if (why !== null || done === null) {
		showAdded(why ?? t("The visual didn't go in."));
		return;
	}
	showAdded({
		note: titleOf(services, project, note),
		open: () => openNote(services, project, note),
		undo: () => undoPlaced(services, project, note, done),
	});
};

/** Makes a note holding only the visual. Shows the toast, or why it couldn't. */
export const addVisualToNewNote = async (
	services: Services,
	project: ProjectId,
	visual: Visual
): Promise<void> => {
	const placing = appendVisual("", visual);
	if ("refused" in placing) {
		showAdded(placing.refused);
		return;
	}
	const { view } = services.view.getState();
	const made = await makeNote(
		services.source,
		view,
		notesReach(view, project),
		project,
		placing.body
	);
	if ("failure" in made) {
		showAdded(made.failure);
		return;
	}
	showAdded({
		// The view may not list it yet, and it's named as it was made.
		note: untitled(view, project),
		open: () => openNote(services, project, made.note),
		undo: () => undoPlaced(services, project, made.note, placing.placed),
	});
};

/**
 * Add to note for a visual in the reply of `agent` or in a note of
 * `project`. It lists the notes of the agent's project, else of `project`,
 * as the menu opens, then New note; none where they're read-only here.
 */
export const addToNoteAction = (
	services: Services,
	{ agent, project }: { readonly agent: AgentId | null; readonly project: ProjectId | null },
	visual: Visual
): VisualAction => ({
	get label() {
		return t("Add to note");
	},
	icon: NotebookPen,
	run: () => {},
	entries: () => {
		const { view } = services.view.getState();
		const target = (agent === null ? null : projectOfAgent(view, agent)) ?? project;
		if (target === null || writerOf(notesReach(view, target)).device === null) return [];
		const rows = notesOf(view, target);
		return [
			...rows.map((row) => ({
				label: row.shown,
				icon: NotebookText,
				run: () => void addVisualToNote(services, target, row.note, visual),
			})),
			{
				label: t("New note"),
				icon: Plus,
				run: () => void addVisualToNewNote(services, target, visual),
				separated: rows.length > 0,
			},
		];
	},
});

/**
 * The card's Add to note, or null where there are no notes to reach, such
 * as outside a reply or a note.
 */
export const useAddToNote = (html: string, title: string): VisualAction | null => {
	const services = useServicesIfAny();
	const origin = useVisualOrigin();
	const here = use(VisualNoteContext);
	const agent = origin?.agent.id ?? null;
	const project = here?.project ?? null;
	return useMemo(
		() =>
			services && (agent !== null || project !== null)
				? addToNoteAction(services, { agent, project }, { html, title })
				: null,
		[services, agent, project, html, title]
	);
};
