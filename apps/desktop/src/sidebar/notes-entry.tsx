// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A project's "Notes" entry in the sidebar: a row that folds, with how many
// notes the project has, and a row per note under it. A click opens the
// note's pane or focuses it, and a right-click offers Rename and Delete,
// which asks first. A right-click on either row offers New note. A visual
// dropped on a note's row goes in as Add to note puts it. The entry shows
// only while the project has a live note on some device.

import { t, useI18n, formatNumber } from "@maxhq/shared/i18n";

import { NotebookText, Pencil, Trash2 } from "lucide-react";
import { type DragEvent, useState } from "react";

import { type Services, useServices, useUi, useView } from "@/app/services";
import { requestConfirm } from "@/components/confirm";
import { RenameInput } from "@/components/rename-input";
import { paneId } from "@/layout/pane-id";
import { cn } from "@/lib/utils";
import { openNote, useActivePane } from "@/notes/open";
import { changeNote } from "@/notes/queries";
import type { Project } from "@maxhq/shared/bindings";
import { deleteNoteQuestion } from "@maxhq/shared/notes/actions";
import { showFailure } from "@maxhq/shared/ui/inbox-notice";
import { transferredVisual } from "@maxhq/shared/notes/visuals";
import { type NoteRow, notesOf, notesReach } from "@maxhq/shared/view/notes";
import { VISUAL_MIME } from "@maxhq/shared/visuals/transfer";
import { showAdded } from "@/visuals/added-toast";
import { addVisualToNote } from "@/visuals/add-to-note";

import { Chevron } from "./agent-row";
import { pressNote } from "./drag";
import { noticesOf } from "./notices";
import { MenuItem, NewNoteItem, RightClickMenu } from "./project-menu";

const ROW =
	"flex h-[26px] w-full shrink-0 items-center gap-2 rounded-md pr-2 text-left text-[12.5px] text-secondary-foreground hover:bg-accent hover:text-foreground";

/** Shows why a change to a note failed, under the sidebar's list. */
const reportFailure = (services: Services, failure: string | null) => {
	if (failure !== null) showFailure(noticesOf(services), failure);
};

const askToDelete = async (services: Services, project: Project, row: NoteRow) => {
	const yes = await requestConfirm(services, {
		...deleteNoteQuestion(row.title),
		action: t("Delete"),
	});
	if (yes)
		reportFailure(
			services,
			await changeNote(services, project.id, row.note, { deleted: true })
		);
};

const NoteItem = ({
	project,
	row,
	active,
	writable,
}: {
	project: Project;
	row: NoteRow;
	active: boolean;
	writable: boolean;
}) => {
	const { t } = useI18n();

	const services = useServices();
	const [renaming, setRenaming] = useState(false);
	const rename = (draft: string) => {
		setRenaming(false);
		const title = draft.trim();
		if (title === "" || title === row.title) return;
		void changeNote(services, project.id, row.note, { title }).then((failure) =>
			reportFailure(services, failure)
		);
	};
	// A visual dragged from a card goes in as Add to note puts it.
	const [over, setOver] = useState(false);
	const dropVisual = {
		onDragOver: (event: DragEvent) => {
			if (!event.dataTransfer.types.includes(VISUAL_MIME)) return;
			event.preventDefault();
			event.dataTransfer.dropEffect = "copy";
			setOver(true);
		},
		onDragLeave: () => setOver(false),
		onDrop: (event: DragEvent) => {
			setOver(false);
			const dropped = transferredVisual(event.dataTransfer);
			if (!dropped) return;
			event.preventDefault();
			if ("refused" in dropped) showAdded(dropped.refused);
			else void addVisualToNote(services, project.id, row.note, dropped.visual);
		},
	};
	if (renaming)
		return (
			<div className={cn(ROW, "bg-accent pl-[36px]")}>
				<RenameInput
					title={row.title}
					label={t("Note title")}
					onCommit={rename}
					onCancel={() => setRenaming(false)}
					className="h-[22px] text-[12.5px]"
				/>
			</div>
		);
	const item = (
		<button
			type="button"
			aria-current={active ? "true" : undefined}
			title={row.shown}
			onClick={(e) => openNote(services, project.id, row.note, { split: e.altKey })}
			onPointerDown={(e) =>
				pressNote(
					services,
					{
						type: "note",
						project: project.id,
						note: row.note,
						title: row.title,
						copy: null,
					},
					row.shown,
					e.nativeEvent
				)
			}
			{...(writable ? dropVisual : {})}
			data-drop-over={over || undefined}
			className={cn(
				ROW,
				"pl-[36px]",
				active && "bg-selected text-foreground",
				over && "bg-accent text-foreground ring-1 ring-ring"
			)}
		>
			<span className="min-w-0 flex-1 truncate">{row.shown}</span>
		</button>
	);
	if (!writable) return item;
	return (
		<RightClickMenu
			items={
				<>
					<NewNoteItem project={project.id} />
					<MenuItem icon={Pencil} label={t("Rename")} onClick={() => setRenaming(true)} />
					<MenuItem
						icon={Trash2}
						label={t("Delete…")}
						onClick={() => void askToDelete(services, project, row)}
					/>
				</>
			}
		>
			{item}
		</RightClickMenu>
	);
};

/** The project's Notes entry, or nothing when it has no live notes. */
export const NotesEntry = ({ project }: { project: Project }) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const rows = notesOf(view, project.id);
	const writable = !notesReach(view, project.id).readOnly;
	const key = `notes:${project.id}`;
	const folded = useUi((s) => s.collapsed.has(key));
	const toggleProject = useUi((s) => s.toggleProject);
	const active = useActivePane();
	if (rows.length === 0) return null;
	return (
		<>
			<RightClickMenu items={writable ? <NewNoteItem project={project.id} /> : null}>
				<button
					type="button"
					aria-expanded={!folded}
					onClick={() => toggleProject(key)}
					className={cn(ROW, "pl-[22px]")}
				>
					<NotebookText aria-hidden size={13} strokeWidth={2} className="shrink-0" />
					<span className="min-w-0 flex-1 truncate">{t("Notes")}</span>
					<span className="shrink-0 text-[11px] text-muted-foreground tabular-nums">
						{formatNumber(rows.length)}
					</span>
					<Chevron closed={folded} />
				</button>
			</RightClickMenu>
			{folded
				? null
				: rows.map((row) => (
						<NoteItem
							key={row.note}
							project={project}
							row={row}
							writable={writable}
							active={
								active ===
								paneId({ kind: "note", project: project.id, note: row.note })
							}
						/>
					))}
		</>
	);
};
