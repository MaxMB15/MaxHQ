// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The files a send copies besides the ones you placed: an excerpt of each
// agent on another device, and a copy of each note from another project,
// both readied on the agent's device as Markdown, the way a pasted file
// goes. And what the error line says when files, excerpts or copies don't
// make it: each reason in its own words, with what to do next.

import type { AgentId, AttachmentId, DeviceId, NoteId, ProjectId } from "../bindings";
import { failureOf } from "../data/failure";
import type { DataSource } from "../data/source";
import { t } from "../i18n";
import { utf8 } from "../notes/merge";
import { farAgents, farNotes, type Segment } from "../pills/draft";
import { sentence } from "../status/format";
import { errorText } from "../timeline/store";
import { noteCopyName, notesReach, shownTitle } from "../view/notes";
import type { ViewState } from "../view/replica";

/** How much of an agent on another device an excerpt takes, in characters. */
export const EXCERPT_BUDGET = 40_000;

// What a file says when staging failed with no reason. Saying it twice
// adds nothing, so the error line leaves it out.
const NO_REASON = "Couldn't get it ready";

const noReason = (error: string | null) =>
	error === null || error === NO_REASON || error === t(NO_REASON);

/** The reason every one of them failed for, as a sentence to add, or nothing when they differ. */
const sharedReason = (errors: readonly (string | null)[]): string => {
	const reasons = new Set(errors.map((error) => (noReason(error) ? NO_REASON : error)));
	const [only] = reasons;
	return reasons.size === 1 && only && only !== NO_REASON && sentence(only) !== ""
		? ` ${sentence(only)}.`
		: "";
};

/** A file that couldn't be readied, and why. */
export type FileFailure = { readonly name: string; readonly error: string | null };

/** What the error line says when a message waits on files that couldn't be readied. */
export const attachFailure = (failed: readonly FileFailure[]): string => {
	const why = sharedReason(failed.map((file) => file.error));
	const [first] = failed;
	return failed.length === 1 && first
		? t("Couldn't attach {{name}}.{{why}} Remove it, then send again.", {
				name: first.name,
				why: why,
			})
		: t("Couldn't attach {{length}} files.{{why}} Remove them, then send again.", {
				length: failed.length,
				why: why,
			});
};

/** An agent on another device whose excerpt couldn't be taken or readied, and why. */
export type ExcerptFailure = { readonly title: string; readonly error: string | null };

/**
 * What the error line says when a message waits on excerpts of agents on
 * other devices that couldn't be taken or readied. Sending again takes
 * them again.
 */
export const excerptFailure = (failed: readonly ExcerptFailure[]): string => {
	const why = sharedReason(failed.map((each) => each.error));
	const [first] = failed;
	return failed.length === 1 && first
		? t("Couldn't get an excerpt of {{title}}.{{why}} Send again, or remove the agent.", {
				title: first.title,
				why: why,
			})
		: t("Couldn't get excerpts of {{length}} agents.{{why}} Send again, or remove them.", {
				length: failed.length,
				why: why,
			});
};

/**
 * What the error line says when a message waits on copies of notes from
 * other projects that couldn't be read or readied. Sending again reads
 * them again.
 */
export const copyFailure = (failed: readonly ExcerptFailure[]): string => {
	const why = sharedReason(failed.map((each) => each.error));
	const [first] = failed;
	return failed.length === 1 && first
		? t("Couldn't copy the note {{title}}.{{why}} Send again, or remove the note.", {
				title: first.title,
				why: why,
			})
		: t("Couldn't copy {{length}} notes.{{why}} Send again, or remove them.", {
				length: failed.length,
				why: why,
			});
};

/** The copies a send goes with, by the pill each is for, or why one isn't ready. */
export type Copies = {
	readonly excerpts: ReadonlyMap<AgentId, AttachmentId>;
	readonly notes: ReadonlyMap<NoteId, AttachmentId>;
	/** What the error line says when one isn't ready, or null. */
	readonly failure: string | null;
};

type Job<K> = {
	readonly id: K;
	readonly title: string;
	readonly take: () => Promise<{ readonly name: string; readonly text: string }>;
};

const messageOf = (error: unknown): string | null =>
	error instanceof Error ? error.message || null : String(error) || null;

/** Takes each copy and readies it on `device`. Gives the ready IDs and the failures. */
const readyAll = async <K>(
	source: Pick<DataSource, "stagePasted">,
	device: DeviceId,
	jobs: readonly Job<K>[]
): Promise<{ ready: Map<K, AttachmentId>; unready: ExcerptFailure[] }> => {
	const ready = new Map<K, AttachmentId>();
	const unready: ExcerptFailure[] = [];
	await Promise.all(
		jobs.map(async ({ id, title, take }) => {
			try {
				const copy = await take();
				const result = await source.stagePasted(
					device,
					copy.name,
					"text/markdown",
					utf8(copy.text)
				);
				if (result.status === "ok" && result.data.status === "staged")
					ready.set(id, result.data.attachment.id);
				else unready.push({ title, error: failureOf(result) || null });
			} catch (error) {
				unready.push({ title, error: messageOf(error) });
			}
		})
	);
	return { ready, unready };
};

/**
 * Takes an excerpt of each agent on another device the draft names, and a
 * copy of each note from a project other than `ownProject`, every note
 * when there's none, and readies them on the agent's `device`.
 */
export const takeCopies = async (
	source: Pick<DataSource, "excerptAgent" | "readNote" | "stagePasted">,
	view: () => ViewState,
	device: DeviceId,
	segments: readonly Segment[],
	ownProject: ProjectId | null
): Promise<Copies> => {
	const agents = farAgents(segments, device).map((ref): Job<AgentId> => ({
		id: ref.agent,
		title: ref.title,
		take: () => source.excerptAgent(ref.agent, EXCERPT_BUDGET),
	}));
	const notes = farNotes(segments, ownProject ?? ("" as ProjectId)).map((ref): Job<NoteId> => ({
		id: ref.note,
		title: shownTitle(ref.title),
		take: async () => {
			const reach = notesReach(view(), ref.project);
			if (reach.device === null) throw new Error(reach.why ?? "");
			const read = await source.readNote(reach.device, ref.project, ref.note, null);
			if (read.status === "error") throw new Error(errorText(read.error));
			if (read.data.deleted) throw new Error(t("It was deleted."));
			return { name: noteCopyName(read.data.title), text: read.data.body };
		},
	}));
	const [excerpts, copies] = await Promise.all([
		readyAll(source, device, agents),
		readyAll(source, device, notes),
	]);
	const failure =
		excerpts.unready.length > 0
			? excerptFailure(excerpts.unready)
			: copies.unready.length > 0
				? copyFailure(copies.unready)
				: null;
	return { excerpts: excerpts.ready, notes: copies.ready, failure };
};
