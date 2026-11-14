// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The number on the app's icon: the agents that need you, as you choose
// what counts. An agent counts once, for the first reason that fits it,
// while it's in the inbox: not settled, not snoozed and not in a hidden
// project. Only lead agents count, as only they alert, since a sub-agent's
// prompt shows on its lead too. A prompt that is an error counts as a
// failure, as its notification does.

import type { StoreApi } from "zustand/vanilla";

import type { Agent, Timestamp } from "../bindings";
import { t } from "../i18n";

import { inboxOf, isSnoozed } from "./inbox";
import { all, type ViewState } from "./replica";
import { hiddenProjects } from "./select";
import type { ViewStore } from "./store";

/** Why an agent counts on the badge. */
export type BadgeReason = "waiting" | "finished" | "failed";

/** The reasons, in the order settings shows them. */
export const BADGE_REASONS: readonly BadgeReason[] = ["waiting", "finished", "failed"];

/** What settings says of each reason. */
export const BADGE_REASON_TEXT: Record<
	BadgeReason,
	{ readonly label: string; readonly detail: string }
> = {
	waiting: {
		get label() {
			return t("Waiting on you");
		},
		get detail() {
			return t("A permission, a question or a consent card");
		},
	},
	finished: {
		get label() {
			return t("Finished");
		},
		get detail() {
			return t("A turn ended and you haven't opened it");
		},
	},
	failed: {
		get label() {
			return t("Failed or stopped with an error");
		},
		get detail() {
			return t("The CLI exited, or a prompt reports an error");
		},
	},
};

/** What the badge shows. */
export type BadgePrefs = {
	/** Whether the icon shows a badge at all. */
	readonly show: boolean;
	/** What counts, in settings order. */
	readonly counts: readonly BadgeReason[];
};

/** The badge shows, and every reason counts. */
export const DEFAULT_BADGE_PREFS: BadgePrefs = { show: true, counts: BADGE_REASONS };

// Bump when the saved shape changes, so old settings read as the defaults.
const VERSION = 1;

/** The settings as kept, with a version. */
export const writeBadgePrefs = (prefs: BadgePrefs): string =>
	JSON.stringify({ version: VERSION, show: prefs.show, counts: prefs.counts });

/** Reads kept settings, defaulting each part that doesn't check out. */
export const readBadgePrefs = (raw: string | null): BadgePrefs => {
	if (raw === null) return DEFAULT_BADGE_PREFS;
	let saved: unknown;
	try {
		saved = JSON.parse(raw);
	} catch {
		return DEFAULT_BADGE_PREFS;
	}
	if (typeof saved !== "object" || saved === null) return DEFAULT_BADGE_PREFS;
	const { version, show, counts } = saved as Record<string, unknown>;
	if (version !== VERSION) return DEFAULT_BADGE_PREFS;
	return {
		show: typeof show === "boolean" ? show : DEFAULT_BADGE_PREFS.show,
		counts: Array.isArray(counts)
			? BADGE_REASONS.filter((reason) => counts.includes(reason))
			: DEFAULT_BADGE_PREFS.counts,
	};
};

/** The reasons with one switched, in settings order. */
export const switchReason = (
	counts: readonly BadgeReason[],
	reason: BadgeReason,
	on: boolean
): BadgeReason[] => BADGE_REASONS.filter((each) => (each === reason ? on : counts.includes(each)));

/** Why the agent would count, before the settings and its inbox have a say. */
export const badgeReason = (view: ViewState, agent: Agent): BadgeReason | null => {
	const { activity } = agent;
	switch (activity.state) {
		case "needs_input":
			return view.tables.attention.get(activity.attention)?.kind === "error"
				? "failed"
				: "waiting";
		case "done_unread":
			return "finished";
		case "error":
			return "failed";
		default:
			return null;
	}
};

/** How many agents count, or 0 when the badge is off. */
export const badgeCount = (view: ViewState, now: Timestamp, prefs: BadgePrefs): number => {
	if (!prefs.show || prefs.counts.length === 0) return 0;
	const hidden = hiddenProjects(view);
	return all(view, "agent").filter((agent) => {
		if (agent.parent !== null) return false;
		const reason = badgeReason(view, agent);
		if (reason === null || !prefs.counts.includes(reason)) return false;
		const inbox = inboxOf(agent);
		if (inbox.settled !== null || isSnoozed(inbox, now)) return false;
		const project = view.tables.workspace.get(agent.workspace)?.project;
		return !(project && hidden.has(project));
	}).length;
};

/** How often the count looks again with no change, for a snooze that ran out. */
export const BADGE_RECHECK_MS = 60_000;

/**
 * Keeps the badge current: `set` hears the count at once, then each time
 * it changes, as the view or the settings change or a snooze runs out. It
 * reads the stores rather than waiting on a render, so a hidden window
 * keeps it current. Returns a function that stops.
 */
export const watchBadge = ({
	view,
	prefs,
	set,
	now = Date.now,
}: {
	view: ViewStore;
	prefs: StoreApi<BadgePrefs>;
	set: (count: number) => void;
	now?: () => Timestamp;
}): (() => void) => {
	let shown: number | null = null;
	const check = () => {
		const count = badgeCount(view.getState().view, now(), prefs.getState());
		if (count === shown) return;
		shown = count;
		set(count);
	};
	check();
	const stopView = view.subscribe(check);
	const stopPrefs = prefs.subscribe(check);
	const timer = setInterval(check, BADGE_RECHECK_MS);
	return () => {
		stopView();
		stopPrefs();
		clearInterval(timer);
	};
};
