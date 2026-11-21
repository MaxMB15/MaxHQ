// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The bell's inbox. Now is what the badge counts, read from the same
// selector, so the two always agree, and the bell's count follows Show the
// badge as the Dock's does. Earlier is the notification log the
// devices keep, newest first, a page at a time, less the notices about
// what an agent in Now still wants, which Now shows. Mark all as read marks
// every notice read and settles what's only unseen, while a prompt stays
// in Now until you answer it. Opening an agent marks its notices read
// through the source, wherever it opens.

import { t } from "../i18n";

import { createStore, type StoreApi } from "zustand/vanilla";

import type { AgentId, Notice, NoticeKind, NoticePage, Timestamp } from "../bindings";
import { describe } from "../data/request-source";
import type { CommandResult, DataSource } from "../data/source";
import { type BadgeItem, type BadgePrefs, type BadgeReason, badgeItems } from "../view/badge";
import type { ViewState } from "../view/replica";
import { attentionOf, deviceName } from "../view/select";

import type { NoticeCalls } from "./requests";

/** How many more notices Show more brings. */
export const NOTICE_PAGE = 20;

/** One row in the bell's popover. */
export type InboxRow = {
	/** Tells rows apart, for a key. */
	readonly key: string;
	/** The agent it's about. */
	readonly agent: AgentId;
	/** The agent's title. */
	readonly title: string;
	/** Its project, or its device when it has none. */
	readonly place: string;
	/** What it wants, or what happened. */
	readonly what: string;
	/** When. */
	readonly at: Timestamp;
	/** Whether it shows the unread dot. */
	readonly unread: boolean;
	/** Why it stays after Mark all as read, if it does. */
	readonly stays?: string;
	/** Whether the agent is gone, so the row can't open it. */
	readonly gone: boolean;
};

/** What each kind of notice says happened. */
export const NOTICE_TEXT: Record<NoticeKind, () => string> = {
	question: () => t("Asking you a question"),
	permission: () => t("Asking for permission"),
	input: () => t("Needs input"),
	error: () => t("Reported an error"),
	finished: () => t("Finished its turn"),
	limit: () => t("Hit a usage limit"),
	resumed: () => t("Waited for the usage limit to reset"),
};

/**
 * The kind of notice the device logged when the agent began to count, or
 * null when it logged none, as for a CLI that exited.
 */
const noticeKindOf = (view: ViewState, { agent, reason }: BadgeItem): NoticeKind | null => {
	if (reason === "finished") return "finished";
	if (agent.activity.state === "error") return null;
	const kind = attentionOf(view, agent)?.kind;
	return kind === "permission" || kind === "question" || kind === "error" ? kind : "input";
};

/** What a Now row says it wants, in the words its activity card uses. */
const nowText = (view: ViewState, item: BadgeItem): string => {
	if (item.reason === "finished") return t("Done, not opened yet");
	const kind = noticeKindOf(view, item);
	return kind ? NOTICE_TEXT[kind]() : t("Stopped with an error");
};

/** Why a reason outlasts Mark all as read. */
const STAYS: Record<BadgeReason, (() => string) | null> = {
	waiting: () => t("Stays until you answer it"),
	failed: () => t("Stays until it runs again"),
	finished: null,
};

/** The agent's project, or its device. */
const placeOf = (view: ViewState, project: string | null | undefined, device: string) =>
	project || deviceName(view, device);

/** The Now rows: exactly what the badge counts for `counts`. */
export const nowRows = (
	view: ViewState,
	now: Timestamp,
	counts: readonly BadgeReason[]
): InboxRow[] =>
	badgeItems(view, now, counts).map((item) => {
		const { agent } = item;
		const workspace = view.tables.workspace.get(agent.workspace);
		const project = workspace?.project ? view.tables.project.get(workspace.project) : undefined;
		return {
			key: `now-${agent.id}`,
			agent: agent.id,
			title: agent.title || t("This agent"),
			place: placeOf(view, project?.name, workspace?.device ?? ""),
			what: nowText(view, item),
			at: item.since,
			unread: true,
			stays: STAYS[item.reason]?.(),
			gone: false,
		};
	});

/**
 * The number on the bell: how many Now rows there are, or null while Show
 * the badge is off, as the bell then shows no count and can't claim that
 * nothing needs you.
 */
export const bellCount = (rows: readonly InboxRow[], prefs: BadgePrefs): number | null =>
	prefs.show ? rows.length : null;

/**
 * Whether a notice is about what an agent in `items` still wants: of the
 * kind it logged then, logged since it began to count.
 */
const stillWanted = (view: ViewState, items: readonly BadgeItem[]) => {
	const live = new Map(items.map((item) => [item.agent.id, item]));
	return (notice: Notice): boolean => {
		const item = live.get(notice.agent);
		return (
			item !== undefined &&
			notice.at >= item.since &&
			notice.kind === noticeKindOf(view, item)
		);
	};
};

/**
 * The Earlier rows, one for each notice that's past. A notice about what
 * an agent in Now, for `counts`, still wants shows only there, so a prompt
 * waiting on you doesn't show twice.
 */
export const earlierRows = (
	view: ViewState,
	notices: readonly Notice[],
	now: Timestamp,
	counts: readonly BadgeReason[]
): InboxRow[] => {
	const live = stillWanted(view, badgeItems(view, now, counts));
	return notices
		.filter((notice) => !live(notice))
		.map((notice) => ({
			key: `${notice.device}-${notice.id}`,
			agent: notice.agent,
			title: notice.title,
			place: placeOf(view, notice.project, notice.device),
			what: notice.summary || NOTICE_TEXT[notice.kind](),
			at: notice.at,
			unread: notice.read_at === null,
			gone: !view.tables.agent.has(notice.agent),
		}));
};

/** Whether the log holds notices past the page. */
export const hasMore = (page: NoticePage): boolean => page.total > page.notices.length;

/** What the popover knows of the log. */
export type NoticeLogState = {
	/** The page shown, or null before the first one came. */
	readonly page: NoticePage | null;
	/** How many notices the page asks for. */
	readonly limit: number;
	/** Why the last call failed, until the next one works. */
	readonly failed: string | null;
};

/** The log's store and the calls that change it. */
export type NoticeLog = {
	readonly store: StoreApi<NoticeLogState>;
	/** Fetches the page again, as many notices as it shows. */
	readonly load: () => Promise<void>;
	/** Fetches a page with {@link NOTICE_PAGE} more. */
	readonly more: () => Promise<void>;
	/**
	 * Marks every notice read, and marks seen each of `finished`, the Now
	 * agents that count only until you look at them.
	 */
	readonly readAll: (finished: readonly AgentId[]) => Promise<void>;
	/** Forgets every notice. */
	readonly clear: () => Promise<void>;
};

/** The finished agents among the Now rows' badge items, for Mark all as read. */
export const finishedAgents = (
	view: ViewState,
	now: Timestamp,
	counts: readonly BadgeReason[]
): AgentId[] =>
	badgeItems(view, now, counts)
		.filter((item) => item.reason === "finished")
		.map((item) => item.agent.id);

/** A log over `calls`, empty until the first load. */
export const createNoticeLog = (
	calls: NoticeCalls & Pick<DataSource, "markSeen">,
	now: () => Timestamp = Date.now
): NoticeLog => {
	const store = createStore<NoticeLogState>(() => ({
		page: null,
		limit: NOTICE_PAGE,
		failed: null,
	}));
	const fetch = async (limit: number) => {
		const result = await calls.listNotices(limit);
		if (result.status === "ok") store.setState({ page: result.data, limit, failed: null });
		else store.setState({ limit, failed: describe(result.error) });
	};
	// Changes the page as the call will, before it answers.
	const change = (edit: (page: NoticePage) => NoticePage) => {
		const { page } = store.getState();
		if (page) store.setState({ page: edit(page) });
	};
	const settle = async (call: Promise<CommandResult>) => {
		const result = await call;
		// A refusal puts back what the devices really hold, and says why.
		if (result.status === "error") {
			await fetch(store.getState().limit);
			store.setState({ failed: describe(result.error) });
		}
	};
	return {
		store,
		load: () => fetch(store.getState().limit),
		more: () => fetch(store.getState().limit + NOTICE_PAGE),
		readAll: async (finished) => {
			const at = now();
			change((page) => ({
				...page,
				notices: page.notices.map((notice) => ({
					...notice,
					read_at: notice.read_at ?? at,
				})),
				unread: 0,
			}));
			for (const agent of finished) void calls.markSeen(agent);
			await settle(calls.readNotices(null));
		},
		clear: async () => {
			change(() => ({ notices: [], total: 0, unread: 0 }));
			await settle(calls.clearNotices());
		},
	};
};
