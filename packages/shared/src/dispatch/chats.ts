// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The dispatcher's chats, the same on every client: the group each falls
// in, how long ago it was active, which match what you search for, where
// the dispatcher moved your last message, and the note a chat saved. A chat
// untouched for a week shows as archived, though its device keeps it as it
// was. Acting on a chat goes through here, so a chat you pick leaves a new
// conversation you started.

import { t } from "../i18n";

import {
	type AgentId,
	type ChatAct,
	DISPATCHER_CHATS_SINCE,
	type DeviceId,
	type DispatcherChat,
	type NoteId,
	type ProjectId,
	type Timestamp,
} from "../bindings";
import type { DataSource } from "../data/source";
import { formatAgoShort } from "../status/format";
import { all, type ViewState } from "../view/replica";
import { linkForDevice } from "../view/select";

import { isWorking } from "./conversation";
import { type DispatchFlow, failureIn, noteOf, tryCall } from "./flow";

const DAY = 24 * 60 * 60 * 1000;

/** How long a chat stays out of Archived while nobody touches it. */
export const ARCHIVE_AFTER_MS = 7 * DAY;

/** Whether the device's `maxd` keeps the dispatcher's chats: protocol 53 or later. */
export const supportsChats = (view: ViewState, device: DeviceId): boolean => {
	const protocol = linkForDevice(view, device)?.protocol ?? null;
	return protocol !== null && protocol >= DISPATCHER_CHATS_SINCE;
};

const newestFirst = (a: DispatcherChat, b: DispatcherChat) => b.active_at - a.active_at;

/** The device's chats, newest first. */
export const chatsOf = (view: ViewState, device: DeviceId | null): DispatcherChat[] =>
	all(view, "dispatcher_chat")
		.filter((chat) => chat.device === device)
		.sort(newestFirst);

/** Whether a chat shows as archived: you archived it, or nobody touched it for a week. */
export const isArchived = (chat: DispatcherChat, now: Timestamp): boolean =>
	chat.archived || now - chat.active_at >= ARCHIVE_AFTER_MS;

/** The groups the chats list shows, in its order. */
export type ChatGroup = "today" | "week" | "earlier" | "archived";

const GROUPS: readonly ChatGroup[] = ["today", "week", "earlier", "archived"];

const startOfDay = (at: Timestamp): Timestamp => new Date(at).setHours(0, 0, 0, 0);

// The week starts on Monday.
const startOfWeek = (at: Timestamp): Timestamp => {
	const day = new Date(startOfDay(at));
	return day.setDate(day.getDate() - ((day.getDay() + 6) % 7));
};

/** The group a chat falls in, by the local calendar. */
export const groupOf = (chat: DispatcherChat, now: Timestamp): ChatGroup => {
	if (isArchived(chat, now)) return "archived";
	if (chat.active_at >= startOfDay(now)) return "today";
	if (chat.active_at >= startOfWeek(now)) return "week";
	return "earlier";
};

/** A group's heading. */
export const groupTitle = (group: ChatGroup | "matches"): string => {
	switch (group) {
		case "today":
			return t("Today");
		case "week":
			return t("This week");
		case "earlier":
			return t("Earlier");
		case "archived":
			return t("Archived");
		case "matches":
			return t("Matches");
	}
};

/** A chat's title, or its number until the dispatcher or you name it. */
export const chatTitle = (chat: DispatcherChat): string =>
	chat.title || t("Chat {{number}}", { number: chat.number });

/** How long ago a chat was active, as a list row shows it, such as "4m" or "2d". */
export const chatAge = (chat: DispatcherChat, now: Timestamp): string =>
	formatAgoShort(chat.active_at, now);

const wordsOf = (query: string): string[] => query.toLowerCase().split(/\s+/).filter(Boolean);

/**
 * How well a chat matches every word: each in its title counts 2, and
 * each only in its summary 1. Null when a word is in neither.
 */
export const chatScore = (chat: DispatcherChat, words: readonly string[]): number | null => {
	const title = chatTitle(chat).toLowerCase();
	const summary = chat.summary.toLowerCase();
	let score = 0;
	for (const word of words) {
		if (title.includes(word)) score += 2;
		else if (summary.includes(word)) score += 1;
		else return null;
	}
	return score;
};

/** The chats that match `query`, the best first, and the newest first on a tie. */
export const searchChats = (chats: readonly DispatcherChat[], query: string): DispatcherChat[] => {
	const words = wordsOf(query);
	return chats
		.map((chat) => ({ chat, score: chatScore(chat, words) }))
		.filter((hit): hit is { chat: DispatcherChat; score: number } => hit.score !== null)
		.sort((a, b) => b.score - a.score || newestFirst(a.chat, b.chat))
		.map((hit) => hit.chat);
};

/** One heading in the chats list and the chats under it. */
export type ChatSection = {
	readonly group: ChatGroup | "matches";
	readonly title: string;
	readonly chats: readonly DispatcherChat[];
};

/**
 * The chats list: by group, each newest first, or while you search, the
 * matches in one list, the best first. An empty group is left out.
 */
export const chatSections = (
	chats: readonly DispatcherChat[],
	now: Timestamp,
	query = ""
): ChatSection[] => {
	if (query.trim()) {
		const found = searchChats(chats, query);
		return found.length > 0
			? [{ group: "matches", title: groupTitle("matches"), chats: found }]
			: [];
	}
	const sorted = [...chats].sort(newestFirst);
	return GROUPS.map((group) => ({
		group,
		title: groupTitle(group),
		chats: sorted.filter((chat) => groupOf(chat, now) === group),
	})).filter((section) => section.chats.length > 0);
};

/**
 * Where the dispatcher moved your last message, as the chat you look at
 * shows it: "to" in the chat it left, "from" in the chat it went to. Keep
 * here acts on `source` either way.
 */
export type MovedMarker = {
	readonly way: "to" | "from";
	/** The chat the message left. */
	readonly source: AgentId;
	/** The other chat. */
	readonly other: DispatcherChat;
};

/** The marker the chat now shows, if the dispatcher moved a message out of it or into it. */
export const movedMarker = (
	chats: readonly DispatcherChat[],
	current: AgentId | null | undefined
): MovedMarker | null => {
	if (!current) return null;
	const here = chats.find((chat) => chat.agent === current);
	if (here?.moved_to) {
		const to = chats.find((chat) => chat.agent === here.moved_to);
		return to ? { way: "to", source: here.agent, other: to } : null;
	}
	const from = chats.find((chat) => chat.moved_to === current);
	return from ? { way: "from", source: from.agent, other: from } : null;
};

/** What the marker says. */
export const movedLine = (marker: MovedMarker): string =>
	marker.way === "to"
		? t("Moved your message to {{title}}", { title: chatTitle(marker.other) })
		: t("Moved your message here from {{title}}", { title: chatTitle(marker.other) });

/** The marker's button, which moves the message back to the chat it left. */
export const movedAction = (marker: MovedMarker): string =>
	marker.way === "to" ? t("Keep here") : t("Move back");

/** What deleting a chat asks first. */
export const deleteChatQuestion = (
	chat: DispatcherChat
): { readonly title: string; readonly detail: string } => ({
	title: t('Delete chat "{{title}}"?', { title: chatTitle(chat) }),
	detail: t(
		"MaxHQ stops it, and the dispatcher won't list or find it again. Its CLI's transcript stays."
	),
});

/** A chat asked to save itself to Notes, as a client watches it. */
export type NoteSave = {
	readonly device: DeviceId;
	readonly chat: AgentId;
	/** The device's notes before, by project and note, to tell the new one. */
	readonly before: ReadonlySet<string>;
	/** Whether the chat's agent worked since, so its stopping ends the save. */
	readonly worked: boolean;
};

/** Where a save stands: the note the chat wrote, over without one, or still going. */
export type NoteSaveStep =
	| { readonly type: "saved"; readonly project: ProjectId; readonly note: NoteId }
	| { readonly type: "ended" }
	| { readonly type: "saving"; readonly save: NoteSave };

/** A save of `chat` to Notes about to be asked for, with the notes there now. */
export const startNoteSave = (view: ViewState, device: DeviceId, chat: AgentId): NoteSave => ({
	device,
	chat,
	before: new Set(
		all(view, "project_notes")
			.filter((notes) => notes.device === device)
			.flatMap((notes) => notes.notes.map((note) => `${notes.project}/${note.note}`))
	),
	worked: false,
});

/**
 * Where `save` stands in `view`. It's over without a note once the chat's
 * agent is gone, or worked and stopped with no question for you.
 */
export const noteSaveStep = (view: ViewState, save: NoteSave): NoteSaveStep => {
	for (const notes of all(view, "project_notes")) {
		if (notes.device !== save.device) continue;
		const made = notes.notes.find(
			(note) =>
				!note.deleted &&
				note.by?.agent === save.chat &&
				!save.before.has(`${notes.project}/${note.note}`)
		);
		if (made) return { type: "saved", project: notes.project, note: made.note };
	}
	const agent = view.tables.agent.get(save.chat);
	if (!agent) return { type: "ended" };
	if (isWorking(agent)) return { type: "saving", save: { ...save, worked: true } };
	return save.worked && agent.activity.state !== "needs_input"
		? { type: "ended" }
		: { type: "saving", save };
};

/** The calls the chats list makes. */
export type ChatCalls = Pick<DataSource, "actOnChat" | "deleteAgent">;

/** What you can do with a chat: what its device does, or delete it. */
export type ChatAction = ChatAct | { readonly type: "delete" };

// The acts that make a chat the conversation now.
const PICKS: ReadonlySet<ChatAction["type"]> = new Set(["pick", "keep_here", "save_note"]);

/**
 * Acts on one of `device`'s chats, or with "delete" deletes it. One that
 * makes a chat the conversation now leaves a new conversation you started.
 * What went wrong shows as the flow's note. Returns whether it went.
 */
export const actOnChat = async (
	flow: DispatchFlow,
	calls: ChatCalls,
	device: DeviceId,
	chat: AgentId,
	action: ChatAction
): Promise<boolean> => {
	const failure = failureIn(
		await tryCall(() =>
			action.type === "delete"
				? calls.deleteAgent(chat)
				: calls.actOnChat(device, chat, action)
		)
	);
	if (failure) {
		flow.store.setState({ note: noteOf(failure) });
		return false;
	}
	if (PICKS.has(action.type)) flow.store.setState({ fresh: false, ran: null, note: null });
	return true;
};
