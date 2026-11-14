// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A project note's pane: a header with where saving stands, History and
// Delete, the title as a field, then the note's editor. It reads and saves
// through this Mac's maxd, or shows another device's copy read-only while
// that's down, with a way to start it. History lists the versions, newest
// first; picking one shows it read-only in place of the editor, and Restore
// saves it as a new version. A note deleted elsewhere says so and offers it
// back.

import { t, useI18n } from "@maxhq/shared/i18n";

import { History, Trash2, X } from "lucide-react";
import {
	type KeyboardEvent,
	useEffect,
	useMemo,
	useRef,
	useState,
	useSyncExternalStore,
} from "react";

import { type Services, useServices, useView } from "@/app/services";
import { requestConfirm } from "@/components/confirm";
import { cn } from "@/lib/utils";
import { Empty } from "@/main-area/empty";
import { LocalMaxdNotice } from "@/main-area/local-maxd";
import { SECONDARY } from "@/settings/controls";
import { VisualNoteContext } from "@/visuals/add-to-note";
import type { NoteId, NoteVersion, NoteVersionInfo, ProjectId } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { formatWhen } from "@maxhq/shared/status/format";
import { noteOn, notesReach, savedBy } from "@maxhq/shared/view/notes";

import { deleteNoteQuestion } from "@maxhq/shared/notes/actions";
import { type Autosave, createAutosave, saveText } from "@maxhq/shared/notes/autosave";
import { useFieldTyping } from "@/composer/typing-prefs";
import { NoteEditor, type NoteEditorHandle } from "./editor";
import { keepOpen, takeFresh } from "./open";
import { useNoteHistory, useNoteVersion } from "./queries";

const HEADER_BUTTON =
	"inline-flex h-6 items-center gap-1.5 rounded-[5px] px-2 text-[12px] text-secondary-foreground hover:bg-accent hover:text-foreground aria-pressed:bg-accent aria-pressed:text-foreground";

/** Asks first, then deletes the note and closes its pane. */
const deleteNote = async (
	services: Services,
	saving: Autosave,
	title: string,
	close: () => void
): Promise<void> => {
	const yes = await requestConfirm(services, {
		...deleteNoteQuestion(title),
		action: t("Delete"),
	});
	if (!yes) return;
	if ((await saving.replace({ deleted: true })) === null) close();
};

/** The versions of a note, newest first. Picking one previews it. */
const HistoryList = ({
	versions,
	failed,
	picked,
	onPick,
}: {
	versions: readonly NoteVersionInfo[] | undefined;
	failed: string | null;
	picked: string | null;
	onPick: (hash: string | null) => void;
}) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const now = useNow(30_000);
	return (
		<aside
			aria-label={t("History")}
			className="flex w-[240px] shrink-0 flex-col gap-0.5 overflow-y-auto border-l border-border p-2"
		>
			<h2 className="px-2 pb-1 text-[11px] font-semibold text-muted-foreground uppercase">
				{t("History")}
			</h2>
			{failed !== null ? (
				<p className="px-2 text-[12px] text-status-error-text">{failed}</p>
			) : !versions ? (
				<p className="px-2 text-[12px] text-muted-foreground">{t("Reading…")}</p>
			) : (
				versions.map((version, index) => {
					const current = index === 0;
					const selected = current ? picked === null : picked === version.hash;
					return (
						<button
							key={version.hash}
							type="button"
							aria-current={selected ? "true" : undefined}
							onClick={() => onPick(current ? null : version.hash)}
							className={cn(
								"flex flex-col items-start rounded-md px-2 py-1 text-left hover:bg-accent",
								selected && "bg-selected"
							)}
						>
							<span className="text-[12.5px] text-foreground tabular-nums">
								{formatWhen(version.at, now)}
								{current ? t(" · Current") : version.deleted ? t(" · Deleted") : ""}
							</span>
							<span className="max-w-full truncate text-[11.5px] text-muted-foreground">
								{savedBy(view, version.by)}
							</span>
						</button>
					);
				})
			)}
		</aside>
	);
};

/** An old version, read-only, with a way to restore it. */
const Preview = ({
	device,
	project,
	note,
	hash,
	readOnly,
	onRestore,
	onBack,
}: {
	device: string;
	project: ProjectId;
	note: NoteId;
	hash: string;
	readOnly: boolean;
	onRestore: (version: NoteVersion) => void;
	onBack: () => void;
}) => {
	const { t } = useI18n();

	const version = useNoteVersion(device, project, note, hash);
	const now = useNow(30_000);
	return (
		<div className="flex min-h-0 flex-1 flex-col">
			<div className="flex shrink-0 items-center gap-2 border-b border-border bg-subpanel px-5 py-2 text-[12.5px] text-secondary-foreground">
				<span className="min-w-0 flex-1 truncate">
					{version.data
						? t('"{{title}}" from {{value2}}', {
								title: version.data.title,
								value2: formatWhen(version.data.at, now),
							})
						: t("Reading the version…")}
				</span>
				<button type="button" className={SECONDARY} onClick={onBack}>
					{t("Back to current")}
				</button>
				{readOnly ? null : (
					<button
						type="button"
						className={SECONDARY}
						disabled={!version.data}
						onClick={() => version.data && onRestore(version.data)}
					>
						{t("Restore")}
					</button>
				)}
			</div>
			<div className="min-h-0 flex-1 overflow-y-auto">
				{version.isError ? (
					<Empty>{version.error.message}</Empty>
				) : version.data ? (
					<NoteEditor
						key={hash}
						initial={version.data.body}
						readOnly
						onChange={() => {}}
					/>
				) : null}
			</div>
		</div>
	);
};

/** An open note, from the version it opened at. */
const NoteBody = ({
	device,
	project,
	note,
	initial,
	readOnly,
	why,
	close,
}: {
	device: string;
	project: ProjectId;
	note: NoteId;
	initial: NoteVersion;
	readOnly: boolean;
	why: string | null;
	close: () => void;
}) => {
	const { t } = useI18n();

	const services = useServices();
	const editor = useRef<NoteEditorHandle>(null);
	const titleField = useRef<HTMLInputElement>(null);
	const fieldTyping = useFieldTyping();
	const [saving] = useState(() =>
		createAutosave({
			initial,
			save: (save) => services.source.saveNote(device, project, note, save),
			read: async (version) => {
				const result = await services.source.readNote(device, project, note, version);
				return result.status === "ok" ? result.data : null;
			},
		})
	);
	const state = useSyncExternalStore(saving.subscribe, saving.snapshot);
	const [showHistory, setShowHistory] = useState(false);
	const [picked, setPicked] = useState<string | null>(null);
	const history = useNoteHistory(device, project, note, state.base, showHistory);
	// Why a visual pasted or dropped in didn't go in, until dismissed.
	const [refused, setRefused] = useState<string | null>(null);
	const here = useMemo(() => ({ project, note }), [project, note]);

	// What waits is saved when the pane closes.
	useEffect(() => {
		const stop = keepOpen(project, note, saving, () => editor.current);
		return () => {
			stop();
			void saving.flush();
		};
	}, [project, note, saving]);

	// Text from elsewhere goes into the editor.
	useEffect(
		() => saving.follow((body, against) => editor.current?.applyIncoming(body, against)),
		[saving]
	);

	// A note just made opens with its title selected, to name it.
	useEffect(() => {
		if (!takeFresh(project, note)) return;
		titleField.current?.focus();
		titleField.current?.select();
	}, [project, note]);

	// The device's head moving, by a save elsewhere or a sync, comes in.
	const head = useView((s) => noteOn(s.view, device, project, note)?.head);
	useEffect(() => {
		if (head) saving.headMoved(head);
	}, [head, saving]);

	const titleKey = (event: KeyboardEvent<HTMLInputElement>) => {
		if (event.key !== "Enter" || event.nativeEvent.isComposing) return;
		event.preventDefault();
		editor.current?.focus();
	};

	if (state.deleted) {
		return (
			<div className="grid h-full place-items-center px-8">
				<div className="flex flex-col items-center gap-3 text-center">
					<p className="text-[13px] text-muted-foreground">
						{t("“{{title}}” was deleted.", { title: state.title })}
					</p>
					{readOnly ? null : (
						<button
							type="button"
							className={SECONDARY}
							onClick={() => void saving.replace({})}
						>
							{t("Bring it back")}
						</button>
					)}
				</div>
			</div>
		);
	}

	return (
		<div className="flex h-full min-h-0 flex-col bg-background">
			<header className="flex h-9 shrink-0 items-center gap-1 border-b border-border px-3">
				<span
					role="status"
					title={why ?? undefined}
					className={cn(
						"min-w-0 flex-1 truncate text-[12px]",
						state.save.kind === "failed" && !readOnly
							? "text-status-error-text"
							: "text-muted-foreground"
					)}
				>
					{saveText(state.save, readOnly)}
				</span>
				<button
					type="button"
					aria-pressed={showHistory}
					className={HEADER_BUTTON}
					onClick={() => {
						setShowHistory((on) => !on);
						setPicked(null);
					}}
				>
					<History aria-hidden size={13} strokeWidth={2} />
					{t("History")}
				</button>
				{readOnly ? null : (
					<button
						type="button"
						className={HEADER_BUTTON}
						onClick={() => void deleteNote(services, saving, state.title, close)}
					>
						<Trash2 aria-hidden size={13} strokeWidth={2} />
						{t("Delete")}
					</button>
				)}
			</header>
			{readOnly && why ? (
				<p className="shrink-0 border-b border-border bg-subpanel px-5 py-1.5 text-[12px] text-muted-foreground">
					{why}
				</p>
			) : null}
			{readOnly ? <LocalMaxdNotice className="mx-5 my-2 shrink-0" /> : null}
			<div className="flex min-h-0 flex-1">
				{picked !== null ? (
					<Preview
						device={device}
						project={project}
						note={note}
						hash={picked}
						readOnly={readOnly}
						onBack={() => setPicked(null)}
						onRestore={(version) => {
							setPicked(null);
							void saving.replace({ title: version.title, body: version.body });
						}}
					/>
				) : null}
				{/* The editor stays while a version shows, so its undo and caret do too. */}
				<div
					className={cn(
						"flex min-h-0 flex-1 flex-col overflow-y-auto",
						picked && "hidden"
					)}
				>
					<input
						ref={titleField}
						value={state.title}
						readOnly={readOnly}
						aria-label={t("Title")}
						placeholder={t("Title")}
						dir="auto"
						{...fieldTyping}
						onChange={(event) => saving.edit({ title: event.target.value })}
						onBlur={() => void saving.flush()}
						onKeyDown={titleKey}
						className="mx-5 mt-5 shrink-0 bg-transparent text-[22px] font-[650] text-foreground-strong outline-none placeholder:text-subtle-foreground"
					/>
					{refused ? (
						<div
							role="alert"
							className="mx-5 mt-3 flex shrink-0 items-center gap-2 rounded-[7px] border border-status-error/30 bg-status-error/8 py-1 pr-1 pl-2.5 text-[12px] text-status-error-text"
						>
							<span className="min-w-0 flex-1">{refused}</span>
							<button
								type="button"
								aria-label={t("Dismiss")}
								className="grid size-[20px] shrink-0 place-items-center rounded-[5px] hover:bg-border"
								onClick={() => setRefused(null)}
							>
								<X aria-hidden size={11} strokeWidth={2.4} />
							</button>
						</div>
					) : null}
					<VisualNoteContext value={here}>
						<NoteEditor
							ref={editor}
							initial={initial.body}
							readOnly={readOnly}
							onChange={(body) => saving.edit({ body })}
							onBlur={() => void saving.flush()}
							onRefuse={setRefused}
						/>
					</VisualNoteContext>
				</div>
				{showHistory ? (
					<HistoryList
						versions={history.data}
						failed={history.isError ? history.error.message : null}
						picked={picked}
						onPick={setPicked}
					/>
				) : null}
			</div>
		</div>
	);
};

/** A note's pane, through the device that reaches its project's notes. */
export const NotePaneFor = ({
	project,
	note,
	close,
}: {
	project: ProjectId;
	note: NoteId;
	close: () => void;
}) => {
	useI18n();

	const view = useView((s) => s.view);
	const reach = useMemo(() => notesReach(view, project), [view, project]);
	if (reach.device === null) {
		return (
			<div className="grid h-full place-items-center px-8">
				<div className="flex max-w-[48ch] flex-col gap-3">
					<p className="text-center text-[13px] text-muted-foreground">{reach.why}</p>
					<LocalMaxdNotice />
				</div>
			</div>
		);
	}
	// Another device's copy is another note's history, so it opens anew.
	return (
		<NoteLoad
			key={reach.device}
			device={reach.device}
			project={project}
			note={note}
			readOnly={reach.readOnly}
			why={reach.why}
			close={close}
		/>
	);
};

const NoteLoad = ({
	device,
	project,
	note,
	readOnly,
	why,
	close,
}: {
	device: string;
	project: ProjectId;
	note: NoteId;
	readOnly: boolean;
	why: string | null;
	close: () => void;
}) => {
	const { t } = useI18n();

	const head = useNoteVersion(device, project, note);
	if (head.isError) return <Empty>{head.error.message}</Empty>;
	if (!head.data) return <Empty>{t("Reading the note…")}</Empty>;
	return (
		<NoteBody
			device={device}
			project={project}
			note={note}
			initial={head.data}
			readOnly={readOnly}
			why={why}
			close={close}
		/>
	);
};
