// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A note's pill, in the composer and in a sent message: what it knows of
// the note now, its first lines read once the preview opens, and a click
// that opens the note's pane. A note that's deleted, or that no device
// has, keeps its label and shows as unresolved.

import { useI18n } from "@maxhq/shared/i18n";

import type { NoteId, ProjectId, Timestamp, VersionHash } from "@maxhq/shared/bindings";
import type { NoteRef } from "@maxhq/shared/pills/draft";
import { firstLines, noteSummary, notesReach, savedBy } from "@maxhq/shared/view/notes";
import type { ComponentProps } from "react";

import { useServices, useView } from "@/app/services";
import { type PillRecord, presentPill } from "@/pills/registry";
import { ShownPill } from "@/pills/shown";

import { openNote } from "./open";
import { useNoteVersion } from "./queries";

/** A note's first lines, read from its version when the preview opens. */
const NoteLines = ({
	project,
	note,
	head,
}: {
	project: ProjectId;
	note: NoteId;
	head: VersionHash;
}) => {
	const { t } = useI18n();

	const device = useView((state) => notesReach(state.view, project).device);
	const read = useNoteVersion(device, project, note, head);
	if (read.isError) return <div className="text-muted-foreground">{t("Couldn't read it")}</div>;
	if (!read.data) return <div className="text-muted-foreground">{t("Reading…")}</div>;
	const lines = firstLines(read.data.body);
	if (lines.length === 0) return <div className="text-muted-foreground">{t("Empty")}</div>;
	return (
		<div className="mt-1.5 space-y-0.5 border-l-2 border-(--pill-note-border) pl-2 text-foreground">
			{lines.map((line, index) => (
				<div key={index} className="truncate">
					{line}
				</div>
			))}
		</div>
	);
};

/** What a note's pill knows of the note now. */
export const useNoteRecord = (ref: NoteRef): Extract<PillRecord, { kind: "note" }> => {
	const view = useView((state) => state.view);
	const found = noteSummary(view, ref.project, ref.note);
	const note = found && !found.deleted ? found : null;
	return {
		kind: "note",
		ref,
		note,
		projectName: view.tables.project.get(ref.project)?.name ?? null,
		savedBy: note ? savedBy(view, note.by) : null,
		lines: note && <NoteLines project={ref.project} note={ref.note} head={note.head} />,
	};
};

/** A note's pill. A click opens the note's pane while it's there. */
export const NotePill = ({
	note: ref,
	now,
	...props
}: Omit<ComponentProps<typeof ShownPill>, "presentation"> & {
	readonly note: NoteRef;
	readonly now: Timestamp;
}) => {
	useI18n();

	const services = useServices();
	const record = useNoteRecord(ref);
	return (
		<ShownPill
			presentation={presentPill(record, { now })}
			{...props}
			{...(record.note
				? {
						className: "pill-action",
						onClick: () => openNote(services, ref.project, ref.note),
					}
				: {})}
		/>
	);
};
