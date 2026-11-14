// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Pills in what you type outside a message: an answer to an approval or a
// question, and a request to the dispatcher. Each goes the way a message's
// text does, with a mark where each pill sits and the pills as refs, and
// maxd hands the agent the markers and block a message gets. Neither
// carries files, so no file goes in one, and an answer can't take an
// agent's excerpt or a note's copy either. A maxd too old for these pills
// gets each one as the words it shows.

import { t } from "../i18n";
import { type AgentId, ANSWER_PILLS_SINCE, type DeviceId, type ProjectId } from "../bindings";
import type { ViewState } from "../view/replica";
import { deviceName, linkForDevice } from "../view/select";

import {
	type Draft,
	type FileFacts,
	farAgents,
	farNotes,
	fromUserInput,
	plainText,
	type Segment,
	toUserInput,
} from "./draft";
import { noteLabel } from "./label";
import type { InlineRef } from "./wire";

/** What you typed: text with a mark where each placed pill sits, and the pills. */
export type Typed = { readonly text: string; readonly refs: readonly InlineRef[] };

/** Nothing typed. */
export const NOTHING_TYPED: Typed = { text: "", refs: [] };

/** A draft as it goes. A file's pill has no place in one, so it drops out. */
export const typedOf = (draft: Draft): Typed => {
	const input = toUserInput(draft, () => null);
	return { text: input.text, refs: input.refs ?? [] };
};

/** What you typed as a draft again, for its field. */
export const draftOf = (typed: Typed): Draft =>
	fromUserInput({ text: typed.text, attachments: [], refs: typed.refs });

/** What you typed with each pill as the words it shows, such as `[Agent: Usage meter]`. */
export const plainOf = (typed: Typed): string =>
	typed.refs.length === 0 ? typed.text : plainText(draftOf(typed).segments, () => undefined);

/**
 * Whether the maxd on `device` takes pills in an answer or a request. One
 * whose version isn't known yet is taken as older.
 */
export const takesAnswerPills = (view: ViewState, device: DeviceId): boolean => {
	const protocol = linkForDevice(view, device)?.protocol ?? null;
	return protocol !== null && protocol >= ANSWER_PILLS_SINCE;
};

/** What you typed as it goes: as typed where pills go, or else as plain words. */
export const typedFor = (typed: Typed, pills: boolean): Typed =>
	pills || typed.refs.length === 0 ? typed : { text: plainOf(typed), refs: [] };

/** What a field says when a file comes its way. */
export const NO_FILES = "Files can't go in here. Send them in a message.";

/**
 * Where a field's pills can point: agents on `device`, the one its agent
 * runs on, or on any device when null, and notes in `project`, its agent's
 * project, or none when null.
 */
export type AnswerPlace = { readonly device: DeviceId | null; readonly project: ProjectId | null };

/** A field whose pills can name an agent on any device, and no notes. */
export const ANY_PLACE: AnswerPlace = { device: null, project: null };

/** Where an answer to `agent` goes, or null while the view doesn't know its workspace. */
export const answerPlace = (view: ViewState, agent: AgentId): AnswerPlace | null => {
	const record = view.tables.agent.get(agent);
	const workspace = record && view.tables.workspace.get(record.workspace);
	return workspace ? { device: workspace.device, project: workspace.project ?? null } : null;
};

/**
 * Why a draft can't go in a field at `place`, or null when it can. An
 * agent on another device goes with an excerpt and a note from another
 * project with a copy, both files, which an answer can't carry.
 */
export const answerBlocked = (
	view: ViewState,
	segments: readonly Segment[],
	place: AnswerPlace
): string | null => {
	const [agent] = place.device === null ? [] : farAgents(segments, place.device);
	if (agent)
		return t(
			"{{value1}} runs on {{value2}}, and an answer can't take its excerpt. Send it in a message.",
			{ value1: agent.title, value2: deviceName(view, agent.device) }
		);
	const [note] = farNotes(segments, place.project ?? "");
	if (note && place.project === null) return "Notes can't go in here. Send them in a message.";
	if (note)
		return t(
			"{{value1}} is in another project, and an answer can't take its copy. Send it in a message.",
			{ value1: noteLabel(note.title) }
		);
	return null;
};

/**
 * A piece of a draft as a field at `place` takes it, such as a paste or a
 * drop: each file, and each agent or note the field can't take, as the
 * words its pill shows, and why the first of them couldn't stay a pill.
 */
export const takenPiece = (
	view: ViewState,
	segments: readonly Segment[],
	place: AnswerPlace,
	factsOf: (key: string) => FileFacts | undefined = () => undefined
): { readonly segments: Segment[]; readonly why: string | null } => {
	let why: string | null = null;
	const taken = segments.map((segment): Segment => {
		if (segment.type !== "pill") return segment;
		const no =
			segment.pill.type === "attachment" ? NO_FILES : answerBlocked(view, [segment], place);
		if (no === null) return segment;
		why ??= no;
		return { type: "text", text: plainText([segment], factsOf) };
	});
	return { segments: taken, why };
};

/** Where maxd's block of context starts in an answer it recorded. */
const BLOCK = "<maxhq_context>";

/**
 * An answer as a card shows it once it's in: what you typed with each
 * pill's marker, without the block maxd added for the agent.
 */
export const shownAnswer = (text: string): string => {
	const at = text.indexOf(BLOCK);
	return at < 0 ? text : text.slice(0, at).trimEnd();
};
