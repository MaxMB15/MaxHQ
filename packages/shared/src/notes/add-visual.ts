// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Add to note from a visual's card, for any client: which project's notes
// it offers, a visual put at the end of a note that isn't open, as a new
// version on its head, a new note holding only the visual, and Undo, which
// takes out exactly the block that went in. Each resolves with what went
// where, or why it couldn't, in words.

import { t } from "../i18n";

import type { AgentId, NoteId, ProjectId } from "../bindings";
import type { DataSource } from "../data/source";
import type { NotesReach } from "../view/notes";
import type { ViewState } from "../view/replica";
import type { Visual } from "../visuals/fence";

import { changeHead, makeNote, writerOf } from "./actions";
import { appendVisual, type Placed, projectOfAgent, takeOut } from "./visuals";

/**
 * The project whose notes Add to note offers for a visual in the reply of
 * `agent` or in a note of `project`: the agent's project, else the note's,
 * with how a client reaches its notes. Null when there's none, or its notes
 * are read-only here.
 */
export const addToNoteTarget = (
	view: ViewState,
	{ agent, project }: { readonly agent: AgentId | null; readonly project: ProjectId | null },
	reachOf: (view: ViewState, project: ProjectId) => NotesReach
): { readonly project: ProjectId; readonly reach: NotesReach } | null => {
	const target = (agent === null ? null : projectOfAgent(view, agent)) ?? project;
	if (target === null) return null;
	const reach = reachOf(view, target);
	return writerOf(reach).device === null ? null : { project: target, reach };
};

/** What a visual put in a note did: what went where, or why it couldn't. */
export type Added = { readonly placed: Placed } | { readonly failure: string };

/** Puts a visual at the end of a note, as a new version on its head. */
export const addToHead = async (
	source: DataSource,
	reach: NotesReach,
	project: ProjectId,
	note: NoteId,
	visual: Visual
): Promise<Added> => {
	// Set in the change, which the type checker can't follow.
	const put: { placed: Placed | null } = { placed: null };
	const why = await changeHead(source, reach, project, note, {
		body: (body) => {
			const placing = appendVisual(body, visual);
			if ("refused" in placing) return placing;
			put.placed = placing.placed;
			return placing.body;
		},
	});
	if (why !== null) return { failure: why };
	return put.placed === null
		? { failure: t("The visual didn't go in.") }
		: { placed: put.placed };
};

/** Makes a note holding only a visual: its ID and what went in, or why it couldn't. */
export const addToNewNote = async (
	source: DataSource,
	view: ViewState,
	reach: NotesReach,
	project: ProjectId,
	visual: Visual
): Promise<{ readonly note: NoteId; readonly placed: Placed } | { readonly failure: string }> => {
	const placing = appendVisual("", visual);
	if ("refused" in placing) return { failure: placing.refused };
	const made = await makeNote(source, view, reach, project, placing.body);
	return "failure" in made ? made : { note: made.note, placed: placing.placed };
};

/** Why Undo can't take a visual out of a note that changed around it. */
export const visualChanged = (): string => t("The visual changed since it went in, so it stays.");

/**
 * Takes what was placed out of a note's head, wherever it is now. Resolves
 * with why it couldn't, or null.
 */
export const takeOutOfHead = (
	source: DataSource,
	reach: NotesReach,
	project: ProjectId,
	note: NoteId,
	placed: Placed
): Promise<string | null> =>
	changeHead(source, reach, project, note, {
		body: (body) => takeOut(body, placed) ?? { refused: visualChanged() },
	});
