// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the desktop's usage popover and the phone's Usage card say beside the
// bars: a meter row read out, how an account stands, when a window resets,
// and why a reading is missing or old. Kept here so both say the same.

import { t } from "../i18n";

import type { Account, Timestamp } from "../bindings";
import type { CommandResult } from "../data/source";
import { errorText } from "../timeline/store";
import type { ViewState } from "../view/replica";
import { accounts, agentsOnAccount, deviceName, waitingOnAccount } from "../view/select";

import {
	formatAgo,
	formatClock,
	formatDollars,
	formatFullWhen,
	formatPercent,
	formatSpanShort,
	formatWhen,
	plural,
	sentence,
} from "./format";

/** A day in milliseconds. */
const DAY = 24 * 60 * 60 * 1000;
import { type Subscription, subscriptions } from "./subscriptions";
import { atLimit, type MeterRow, type WindowView, windowNoun } from "./usage";

/** What a screen reader hears for one meter row, such as "Codex at its limit". */
export const meterRowLabel = (row: MeterRow): string => {
	if (row.percent === null) return t("{{row_label}} not checked yet", { row_label: row.label });
	if (row.usedUp) return t("{{row_label}} at its limit", { row_label: row.label });
	const week =
		row.weekPercent === null
			? ""
			: t(", {{formatPercent_row_weekPercent}} of the week", {
					formatPercent_row_weekPercent: formatPercent(row.weekPercent),
				});
	return t("{{value1}} {{value2}} used{{value3}}", {
		value1: row.label,
		value2: formatPercent(row.percent),
		value3: week,
	});
};

/** The right side of a plan's heading, counting the agents of all its logins. */
export const accountStatus = (view: ViewState, plan: Subscription): string => {
	const logins = plan.logins.map((login) => login.id);
	if (atLimit(plan.account)) {
		const waiting = waitingOnAccount(view, plan.account, logins);
		return waiting > 0
			? t("At limit, {{plural_waiting___agent}} waiting", {
					plural_waiting___agent: plural(waiting, "agent"),
				})
			: t("At limit");
	}
	return plural(logins.flatMap((login) => agentsOnAccount(view, login)).length, "agent");
};

/** The line under a 5-hour window that has nothing left because the week ran out. */
export const weekNote = (windows: readonly WindowView[], now: Timestamp): string | undefined => {
	const blocked = windows.find((w) => w.blockedByWeek);
	if (!blocked) return undefined;
	const until =
		blocked.resetsAt !== null
			? t(" until {{value1}}", { value1: formatWhen(blocked.resetsAt, now) })
			: "";
	return t(
		"The week is used up, so the {{windowNoun_blocked_minutes}} has nothing left{{until}}.",
		{ windowNoun_blocked_minutes: windowNoun(blocked.minutes), until: until }
	);
};

/**
 * What the reset column says for a window: how long until it resets and the
 * clock time when that's under a day, such as "resets in 2h, 15:30", and the
 * day and time further out, such as "resets Thu 02:00".
 */
export const resetText = (window: WindowView, now: Timestamp): string => {
	if (window.spend) {
		return t("{{value1}} of {{value2}}", {
			value1: formatDollars(window.spend.used_cents),
			value2: formatDollars(window.spend.limit_cents),
		});
	}
	if (window.percent === null) return t("Not reported");
	if (window.resetsAt === null) return "";
	const left = window.resetsAt - now;
	if (left > 0 && left < DAY)
		return t("resets in {{span}}, {{time}}", {
			span: formatSpanShort(left),
			time: formatClock(window.resetsAt),
		});
	return t("resets {{formatWhen_window_resetsAt__now}}", {
		formatWhen_window_resetsAt__now: formatWhen(window.resetsAt, now),
	});
};

/** The full day and time a window resets, for the reset column's tooltip. */
export const resetTitle = (window: WindowView): string | undefined =>
	window.spend || window.resetsAt === null ? undefined : formatFullWhen(window.resetsAt);

/** The line about a failed check, when the newest check failed, or about no check yet. */
export const usageFailureNote = (account: Account, now: Timestamp): string | undefined => {
	const failure = account.usage_failure;
	const usage = account.usage;
	if (!usage && !failure && account.vendor === "cursor") {
		return t("Turn on Cursor usage in a Mac's device settings to see this month here.");
	}
	// Nothing checks an ACP agent's usage, so it never comes.
	if (!usage && !failure && account.vendor === "acp") return t("Doesn't report usage.");
	if (!usage && !failure) return t("Not checked yet.");
	if (!failure || (usage && failure.at <= usage.checked_at)) return undefined;
	if (!usage)
		return t("Couldn't check usage. {{sentence_failure_reason}}.", {
			sentence_failure_reason: sentence(failure.reason),
		});
	return t(
		"The last check {{formatAgo_failure_at__now}} failed. {{sentence_failure_reason}}. These figures are from {{formatWhen_usage_checked_at__now}}.",
		{
			formatAgo_failure_at__now: formatAgo(failure.at, now),
			sentence_failure_reason: sentence(failure.reason),
			formatWhen_usage_checked_at__now: formatWhen(usage.checked_at, now),
		}
	);
};

/**
 * The line under a plan: where its usage shows for a token or sign-in whose
 * own login isn't in sight, and otherwise how its last check went.
 */
export const planNote = (
	view: ViewState,
	plan: Subscription,
	now: Timestamp
): string | undefined => {
	if (!plan.borrowed) return usageFailureNote(plan.account, now);
	const device = deviceName(view, plan.account.device);
	switch (plan.account.login) {
		case "chat_gpt_sign_in":
			return t(
				"Signed in on {{device}}. Its usage shows with a Mac's Codex login on the same email.",
				{ device }
			);
		case "cursor_api_key":
			return t(
				"Signed in on {{device}} with an API key. Its month shows with a Mac's Cursor login.",
				{ device }
			);
		default:
			return t(
				"Signed in on {{device}} with a token. Its usage shows with the login it came from.",
				{ device }
			);
	}
};

/** What a check you asked for says when it couldn't go out, or null. */
export const usageCheckFailure = (result: CommandResult): string | null =>
	result.status === "ok"
		? null
		: t("Couldn't check usage. {{errorText_result_error}}", {
				errorText_result_error: errorText(result.error),
			});

/**
 * The plans the phone's Usage card lists: Claude and Codex, and Cursor once
 * its device's Cursor usage switch is on. The phone has no dashboard link to
 * fall back on, so a Cursor login without it stays out.
 */
export const usagePlans = (view: ViewState): Subscription[] =>
	subscriptions(
		accounts(view).filter(
			(account) =>
				account.vendor === "claude" ||
				account.vendor === "codex" ||
				(account.vendor === "cursor" &&
					view.tables.device_settings.get(account.device)?.cursor_usage === true)
		)
	);
