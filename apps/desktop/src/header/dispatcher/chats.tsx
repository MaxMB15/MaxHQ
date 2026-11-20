// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The dispatcher's chats in its panel: the buttons that list them and save
// the one you're in to Notes, the list with its search, and the bar that
// says where the dispatcher moved your last message. A row picks its chat,
// and renames, saves, archives or deletes it. Save to Notes asks the chat
// to write the note, and opens it once it's there. The list lies inside
// the panel, so using it never closes the panel.

import { useI18n } from "@maxhq/shared/i18n";

import { Archive, ArchiveRestore, NotebookPen, Pencil, Trash2 } from "lucide-react";
import { type KeyboardEvent, useEffect, useMemo, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { ICON_BUTTON } from "@/agent-view/find/bar";
import { useServices, useView } from "@/app/services";
import { requestConfirm } from "@/components/confirm";
import { RenameInput } from "@/components/rename-input";
import { openNote } from "@/notes/open";
import type { AgentId, DeviceId, DispatcherChat, Timestamp } from "@maxhq/shared/bindings";
import {
	actOnChat,
	chatAge,
	chatSections,
	chatsOf,
	type ChatAction,
	chatTitle,
	deleteChatQuestion,
	isArchived,
	movedAction,
	movedLine,
	movedMarker,
	noteSaveStep,
	startNoteSave,
	supportsChats,
} from "@maxhq/shared/dispatch/chats";
import type { DispatchFlow } from "@maxhq/shared/dispatch/flow";
import { useNow } from "@maxhq/shared/lib/clock";
import { all } from "@maxhq/shared/view/replica";

/** Whether the panel lists the chats, and the chat saving to Notes. */
export type ChatPanel = {
	readonly listing: boolean;
	readonly setListing: (listing: boolean) => void;
	/** The chat asked to save itself to Notes, until its note is there or it stops. */
	readonly saving: AgentId | null;
	/** Asks a chat to save itself to Notes, and opens the note once it's there. */
	readonly save: (chat: AgentId) => void;
};

/**
 * The panel's chats: whether it lists them, and Save to Notes, which opens
 * the note the chat writes and closes the panel with `setOpen(false)`.
 */
export const useChatPanel = (
	flow: DispatchFlow,
	device: DeviceId | null,
	setOpen: (open: boolean) => void
): ChatPanel => {
	const services = useServices();
	const [listing, setListing] = useState(false);
	const [saving, setSaving] = useState<AgentId | null>(null);
	// Stops watching the save in progress.
	const watch = useRef<(() => void) | null>(null);
	useEffect(() => () => watch.current?.(), []);

	// Ends a save's watch, unless a later save took its place.
	const end = (stop: () => void) => {
		stop();
		if (watch.current !== stop) return;
		watch.current = null;
		setSaving(null);
	};

	const save = (chat: AgentId) => {
		if (!device) return;
		// Watch from before the call, as the note can come with its answer.
		let step = startNoteSave(services.view.getState().view, device, chat);
		const stop = services.view.subscribe(({ view }) => {
			const next = noteSaveStep(view, step);
			if (next.type === "saving") {
				step = next.save;
				return;
			}
			end(stop);
			if (next.type === "ended") return;
			setOpen(false);
			openNote(services, next.project, next.note);
		});
		watch.current?.();
		watch.current = stop;
		setSaving(chat);
		setListing(false);
		void actOnChat(flow, services.source, device, chat, { type: "save_note" }).then((went) => {
			if (!went) end(stop);
		});
	};

	return { listing, setListing, saving, save };
};

/** The top bar's Chats and Save to Notes, on a device that keeps chats. */
export const ChatButtons = ({
	panel,
	device,
	current,
	className,
}: {
	panel: ChatPanel;
	device: DeviceId | null;
	/** The chat you're in, if any. */
	current: AgentId | undefined;
	className: string;
}) => {
	const { t } = useI18n();

	const supported = useView((s) => (device ? supportsChats(s.view, device) : false));
	if (!supported) return null;
	const saving = current !== undefined && panel.saving === current;
	return (
		<>
			<button
				type="button"
				className={className}
				disabled={!current || saving}
				onClick={() => {
					if (current) panel.save(current);
				}}
			>
				{saving ? t("Saving to Notes") : t("Save to Notes")}
			</button>
			<button
				type="button"
				className={className}
				aria-pressed={panel.listing}
				onClick={() => panel.setListing(!panel.listing)}
			>
				{t("Chats")}
			</button>
		</>
	);
};

/** Where the dispatcher moved your last message, and the button that moves it back. */
export const MovedBar = ({
	flow,
	device,
	current,
}: {
	flow: DispatchFlow;
	device: DeviceId | null;
	current: AgentId;
}) => {
	useI18n();
	const services = useServices();
	const marker = useView(useShallow((s) => movedMarker(all(s.view, "dispatcher_chat"), current)));
	if (!marker || !device) return null;
	return (
		<div
			role="status"
			className="flex shrink-0 items-center gap-2 border-b border-sidebar-border bg-subpanel py-1 pr-2 pl-4 text-[12px] text-muted-foreground"
		>
			<span className="min-w-0 flex-1 truncate">{movedLine(marker)}</span>
			<button
				type="button"
				className="h-[24px] shrink-0 rounded-[6px] px-2 text-[12px] font-[500] text-foreground hover:bg-secondary"
				onClick={() =>
					void actOnChat(flow, services.source, device, marker.source, {
						type: "keep_here",
					})
				}
			>
				{movedAction(marker)}
			</button>
		</div>
	);
};

/** One chat in the list: its title, age and summary, and what you can do with it. */
const ChatRow = ({
	chat,
	now,
	current,
	renaming,
	saving,
	onPick,
	onRename,
	onRenamed,
	onSave,
	onArchive,
	onDelete,
}: {
	chat: DispatcherChat;
	now: Timestamp;
	current: boolean;
	renaming: boolean;
	saving: boolean;
	onPick: () => void;
	onRename: () => void;
	onRenamed: (title: string | null) => void;
	onSave: () => void;
	onArchive: () => void;
	onDelete: () => void;
}) => {
	const { t } = useI18n();

	const title = chatTitle(chat);
	const archived = isArchived(chat, now);
	if (renaming)
		return (
			<li className="flex h-[30px] items-center px-2">
				<RenameInput
					title={chat.title}
					label={t("Rename chat")}
					onCommit={(draft) => onRenamed(draft)}
					onCancel={() => onRenamed(null)}
					className="h-[24px] text-[12.5px]"
				/>
			</li>
		);
	return (
		<li className="group relative">
			<button
				type="button"
				aria-current={current ? "true" : undefined}
				onClick={onPick}
				className="flex w-full min-w-0 flex-col gap-0.5 rounded-[6px] px-2 py-1.5 text-left outline-none hover:bg-secondary focus-visible:ring-2 focus-visible:ring-ring aria-[current=true]:bg-secondary"
			>
				<span className="flex min-w-0 items-baseline gap-2">
					<span className="min-w-0 flex-1 truncate text-[12.5px] text-foreground">
						{title}
					</span>
					<span className="shrink-0 text-[11px] text-muted-foreground tabular-nums">
						{chatAge(chat, now)}
					</span>
				</span>
				{chat.summary ? (
					<span className="truncate text-[11.5px] text-muted-foreground">
						{chat.summary}
					</span>
				) : null}
			</button>
			<span className="absolute top-1 right-1 hidden items-center gap-0.5 rounded-[6px] bg-secondary pl-1 group-focus-within:flex group-hover:flex">
				<button
					type="button"
					className={ICON_BUTTON}
					aria-label={t("Rename")}
					title={t("Rename")}
					onClick={onRename}
				>
					<Pencil size={13} aria-hidden />
				</button>
				<button
					type="button"
					className={ICON_BUTTON}
					aria-label={t("Save to Notes")}
					title={t("Save to Notes")}
					disabled={saving}
					onClick={onSave}
				>
					<NotebookPen size={13} aria-hidden />
				</button>
				<button
					type="button"
					className={ICON_BUTTON}
					aria-label={archived ? t("Unarchive") : t("Archive")}
					title={archived ? t("Unarchive") : t("Archive")}
					onClick={onArchive}
				>
					{archived ? (
						<ArchiveRestore size={13} aria-hidden />
					) : (
						<Archive size={13} aria-hidden />
					)}
				</button>
				<button
					type="button"
					className={ICON_BUTTON}
					aria-label={t("Delete")}
					title={t("Delete")}
					onClick={onDelete}
				>
					<Trash2 size={13} aria-hidden />
				</button>
			</span>
		</li>
	);
};

/**
 * The device's chats, searchable, grouped as Today, This week, Earlier and
 * Archived. Picking one makes it the chat you're in. Esc clears the search,
 * then closes the list.
 */
export const ChatsList = ({
	flow,
	device,
	current,
	panel,
}: {
	flow: DispatchFlow;
	device: DeviceId;
	/** The chat you're in, if any. */
	current: AgentId | undefined;
	panel: ChatPanel;
}) => {
	const { t } = useI18n();

	const services = useServices();
	const chats = useView(useShallow((s) => chatsOf(s.view, device)));
	const now = useNow(60_000);
	const [query, setQuery] = useState("");
	const [renaming, setRenaming] = useState<AgentId | null>(null);
	const sections = useMemo(() => chatSections(chats, now, query), [chats, now, query]);

	const act = (chat: AgentId, action: ChatAction) =>
		actOnChat(flow, services.source, device, chat, action);

	const pick = async (chat: AgentId) => {
		if (chat === current || (await act(chat, { type: "pick" }))) panel.setListing(false);
	};

	const rename = (chat: DispatcherChat, draft: string | null) => {
		setRenaming(null);
		const title = draft?.trim();
		if (title && title !== chat.title) void act(chat.agent, { type: "rename", title });
	};

	const remove = async (chat: DispatcherChat) => {
		const question = deleteChatQuestion(chat);
		if (await requestConfirm(services, { ...question, action: t("Delete") }))
			await act(chat.agent, { type: "delete" });
	};

	const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
		if (e.key === "Escape") {
			e.preventDefault();
			if (query) setQuery("");
			else panel.setListing(false);
		} else if (e.key === "Enter") {
			e.preventDefault();
			const first = sections[0]?.chats[0];
			if (first) void pick(first.agent);
		}
	};

	return (
		<div className="flex min-h-0 flex-1 flex-col">
			<div className="shrink-0 px-3 pt-2 pb-1">
				<input
					type="search"
					autoFocus
					value={query}
					aria-label={t("Search chats")}
					placeholder={t("Search chats")}
					spellCheck={false}
					autoComplete="off"
					onChange={(e) => setQuery(e.target.value)}
					onKeyDown={onKeyDown}
					className="h-[28px] w-full rounded-[6px] border border-input bg-card px-2 text-[12.5px] text-foreground outline-none placeholder:text-muted-foreground focus:border-ring"
				/>
			</div>
			<div className="min-h-0 flex-1 overflow-y-auto px-2 pb-2">
				{sections.length === 0 ? (
					<p className="px-2 py-3 text-[12px] text-muted-foreground">
						{query.trim() ? t("No chats match") : t("No chats yet")}
					</p>
				) : (
					sections.map((section) => (
						<section key={section.group} aria-label={section.title}>
							<h3 className="px-2 pt-2 pb-1 text-[11px] font-[600] text-muted-foreground">
								{section.title}
							</h3>
							<ul className="flex flex-col">
								{section.chats.map((chat) => (
									<ChatRow
										key={chat.agent}
										chat={chat}
										now={now}
										current={chat.agent === current}
										renaming={renaming === chat.agent}
										saving={panel.saving === chat.agent}
										onPick={() => void pick(chat.agent)}
										onRename={() => setRenaming(chat.agent)}
										onRenamed={(title) => rename(chat, title)}
										onSave={() => panel.save(chat.agent)}
										onArchive={() =>
											void act(chat.agent, {
												type: "archive",
												archived: !isArchived(chat, now),
											})
										}
										onDelete={() => void remove(chat)}
									/>
								))}
							</ul>
						</section>
					))
				)}
			</div>
		</div>
	);
};

/** Whether a pointer went down in a confirmation, which leaves the panel open. */
export const inDialog = (target: EventTarget | null): boolean =>
	target instanceof Element && target.closest("[role=alertdialog]") !== null;
