// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Plan usage as the header meter and the usage popover show it. Two rules
// sit on top of the vendor's numbers: a window at 99% or more counts as used
// up, and once the weekly window is used up the 5-hour window has nothing
// left either, whatever its own figure says. Cursor's month meters follow the
// billing cycle instead, and show beside the rolling windows without either
// rule.

import { t } from "../i18n";

import { isLocalAccount, kindName, VENDOR_NAME } from "./agent-kind";
import { plural, formatPercent } from "./format";
import { subscriptions } from "./subscriptions";
import type { Account, Timestamp, UsageSpend, UsageWindow, Vendor } from "../bindings";

/** A window at or above this percentage counts as used up. */
export const USED_UP_PERCENT = 99;

/** The 5-hour window's length in minutes. */
export const FIVE_HOURS = 300;

/** The weekly window's length in minutes. */
export const WEEK = 7 * 24 * 60;

/** Whether a window has nothing left. */
export const isUsedUp = (window: Pick<UsageWindow, "used_percent">): boolean =>
	window.used_percent !== null && window.used_percent >= USED_UP_PERCENT;

/** One window, ready to draw. */
export type WindowView = {
	/** The window's name, such as "5 hours" or "Week". */
	readonly label: string;
	/** The window's length in minutes. */
	readonly minutes: number;
	/** How much is used, from 0 to 100, or null when the vendor didn't say. */
	readonly percent: number | null;
	/** Whether nothing is left in it. */
	readonly usedUp: boolean;
	/** When the window frees up again, if known. */
	readonly resetsAt: Timestamp | null;
	/** Set when the weekly window is the reason this one has nothing left. */
	readonly blockedByWeek: boolean;
	/** What a limit set in money has spent, if it is one. */
	readonly spend: UsageSpend | null;
};

/** The name a window goes by, from its length. */
export const windowLabel = (minutes: number): string => {
	if (minutes === FIVE_HOURS) return plural(5, "hour");
	if (minutes === WEEK) return t("Week");
	if (minutes % (24 * 60) === 0) return plural(minutes / (24 * 60), "day");
	if (minutes % 60 === 0) return plural(minutes / 60, "hour");
	return plural(minutes, "min", "min");
};

/** A window's name in a narrow row, such as "5h" or "wk". */
export const windowShort = (minutes: number): string => {
	if (minutes === WEEK) return t("wk");
	if (minutes % (24 * 60) === 0) return t("{{count, number}}d", { count: minutes / (24 * 60) });
	if (minutes % 60 === 0) return t("{{count, number}}h", { count: minutes / 60 });
	return t("{{count, number}}m", { count: minutes });
};

/** A window's name inside a sentence, such as "5-hour window". */
export const windowNoun = (minutes: number): string => {
	if (minutes === WEEK) return t("weekly window");
	if (minutes % (24 * 60) === 0)
		return t("{{minutes____24___60}}-day window", { minutes____24___60: minutes / (24 * 60) });
	if (minutes % 60 === 0)
		return t("{{minutes___60}}-hour window", { minutes___60: minutes / 60 });
	return t("{{minutes}}-minute window", { minutes: minutes });
};

/** Whether a window follows the billing cycle rather than rolling. */
const isCycle = (w: UsageWindow): boolean => w.period === "billing_cycle";

/**
 * The account's own rolling windows, shortest first, with the two rules
 * applied. Windows that cover only part of the account, such as one model's
 * limit, and billing cycle meters are left out.
 */
export const accountWindows = (account: Pick<Account, "usage">): WindowView[] => {
	const own = (account.usage?.windows ?? [])
		.filter((w) => w.scope === null && !isCycle(w))
		.sort((a, b) => a.window_minutes - b.window_minutes);
	const week = own.find((w) => w.window_minutes >= WEEK && isUsedUp(w));
	return own.map((w) => {
		const blockedByWeek = week !== undefined && w.window_minutes < week.window_minutes;
		return {
			label: windowLabel(w.window_minutes),
			minutes: w.window_minutes,
			percent: blockedByWeek ? 100 : w.used_percent,
			usedUp: blockedByWeek || isUsedUp(w),
			resetsAt: blockedByWeek ? week.resets_at : w.resets_at,
			blockedByWeek,
			spend: null,
		};
	});
};

/** What each part of a billing cycle is called. The whole plan has no scope. */
const CYCLE_LABEL: Record<string, string> = {
	get ""() {
		return t("Total");
	},
	get auto() {
		return t("Auto");
	},
	api: "API",
	get on_demand() {
		return t("On-demand");
	},
};

/** The billing cycle's meters, such as Cursor's Total, Auto and API, in the order `maxd` sent them. */
export const cycleWindows = (account: Pick<Account, "usage">): WindowView[] =>
	(account.usage?.windows ?? []).filter(isCycle).map((w) => ({
		label: CYCLE_LABEL[w.scope ?? ""] ?? w.scope!,
		minutes: w.window_minutes,
		percent: w.used_percent,
		usedUp: isUsedUp(w),
		resetsAt: w.resets_at,
		blockedByWeek: false,
		spend: w.spend ?? null,
	}));

/** Where Cursor shows the plan's usage. */
export const CURSOR_USAGE_URL = "https://cursor.com/dashboard";

/** The 5-hour window with the rules applied, or the shortest one there is. */
export const shortWindow = (account: Pick<Account, "usage">): WindowView | undefined => {
	const windows = accountWindows(account);
	return windows.find((w) => w.minutes === FIVE_HOURS) ?? windows[0];
};

/** Whether any of the account's own windows is used up. */
export const atLimit = (account: Pick<Account, "usage">): boolean =>
	accountWindows(account).some((w) => w.usedUp);

/** The used-up window that frees up last, which is when the account can work again. */
export const blockingWindow = (account: Pick<Account, "usage">): WindowView | undefined =>
	accountWindows(account)
		.filter((w) => w.usedUp)
		.sort((a, b) => b.minutes - a.minutes)[0];

export { VENDOR_NAME };

/**
 * The account's name as its plan reads, such as "Claude Max" or "Codex
 * Plus". Without a reported plan it falls back to the product alone, since
 * the app never guesses a tier. A local ACP agent goes by the name you gave
 * it, and a local model's account is Local, never Claude.
 */
export const accountName = (
	account: Pick<Account, "vendor" | "plan" | "label"> & Partial<Pick<Account, "login">>
): string => {
	if (isLocalAccount(account)) return kindName("local");
	if (account.vendor === "acp") return account.label;
	const plan = account.plan?.trim();
	if (!plan) return VENDOR_NAME[account.vendor];
	const words = plan
		.split(/[_\s]+/)
		.filter(Boolean)
		.map((w) => w[0]!.toUpperCase() + w.slice(1));
	return `${VENDOR_NAME[account.vendor]} ${words.join(" ")}`;
};

/** One row of the header meter. */
export type MeterRow = {
	/** The vendor the row is for. */
	readonly vendor: Vendor;
	/** "Claude" or "Codex". */
	readonly label: string;
	/** The 5-hour figure with the rules applied, or null before any reading. */
	readonly percent: number | null;
	/** Whether the row shows a used-up account. */
	readonly usedUp: boolean;
	/** The same account's weekly figure, or null when it has no week or no reading. */
	readonly weekPercent: number | null;
};

/**
 * One meter row per vendor that has an account, each showing the plan with
 * the least left in its 5-hour window when there are several, and that same
 * plan's week. The plans are the usage popover's.
 */
export const meterRows = (accounts: readonly Account[]): MeterRow[] => {
	const vendors: Vendor[] = ["claude", "codex"];
	const plans = subscriptions(accounts).map((p) => p.account);
	return vendors.flatMap((vendor) => {
		const mine = plans.filter((a) => a.vendor === vendor);
		if (mine.length === 0) return [];
		const fullest = mine
			.map((a) => ({ short: shortWindow(a), windows: accountWindows(a) }))
			.filter((p): p is { short: WindowView; windows: WindowView[] } => p.short !== undefined)
			.sort((a, b) => (b.short.percent ?? -1) - (a.short.percent ?? -1))[0];
		return [
			{
				vendor,
				label: VENDOR_NAME[vendor],
				percent: fullest?.short.percent ?? null,
				usedUp: fullest?.short.usedUp ?? false,
				weekPercent: fullest?.windows.find((w) => w.minutes === WEEK)?.percent ?? null,
			},
		];
	});
};

/** The account with the least left in its 5-hour window, as the agent hover shows it. */
export const accountSummary = (account: Account): string => {
	const window = shortWindow(account);
	const name = accountName(account);
	if (!window || window.percent === null) return name;
	if (window.usedUp)
		return t("{{name}}, at its {{window_blockedByWeek____weekly_____5_hour}} limit", {
			name: name,
			window_blockedByWeek____weekly_____5_hour: window.blockedByWeek
				? t("weekly")
				: t("5-hour"),
		});
	return t("{{name}}, {{percent}} of {{window}}", {
		name,
		percent: formatPercent(window.percent),
		window: window.label.toLowerCase(),
	});
};
