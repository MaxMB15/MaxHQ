// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A cloud model server's credits, as the usage popover, the server's row in
// Settings and the model picker show them. OpenRouter reports what the key
// `maxd` holds has spent and, for a key with a limit, what's left of it.
// With an admin key, `maxd` reads the account's balance too, which then
// leads, with the key's spending second. A failed read keeps the last
// figures, drawn dimmed, with why in the tooltip. Money shows in the
// interface's locale, always with cents.

import { t } from "../i18n";

import {
	type DeviceId,
	type ModelAction,
	type ModelServer,
	type ModelServerId,
	type ServerCredits,
	type Timestamp,
} from "../bindings";
import { formatAgo, formatDollars, formatWhen } from "../status/format";
import type { WindowView } from "../status/usage";
import { all, type ViewState } from "../view/replica";
import { isOpenRouterUrl } from "./openrouter";
import { takesCloudServers } from "./select";

/** Where OpenRouter makes management keys, the admin keys MaxHQ reads the balance with. */
export const ADMIN_KEYS_URL = "https://openrouter.ai/settings/management-keys";

/** Money as the credits show it, always with cents, such as "$10.00". */
const money = (cents: number): string => formatDollars(cents, true);

/** How much of `total` is used, from 0 to 100, when `left` of it is left. */
const usedPercent = (left: number, total: number): number =>
	total > 0 ? Math.min(100, Math.max(0, ((total - left) / total) * 100)) : 100;

/** One bar of a server's credits, drawn the way a usage window is. */
export type CreditsBar = {
	/** The bar as the usage rows draw it. `label` names the row. */
	readonly window: WindowView;
	/** The figures beside the bar, such as "$7.42 left of $10.00" or "$2.58 spent". */
	readonly text: string;
};

/** A server's credits, ready to draw. */
export type CreditsView = {
	/** The bars: the account's balance with an admin key, else the key's. */
	readonly bars: readonly CreditsBar[];
	/** What comes second, such as "This key: $2.58" or "Resets monthly", or "". */
	readonly status: string;
	/** All of it in one line, for the server's row in Settings. */
	readonly line: string;
	/** The main figure alone, such as "$7.42 left", for the model picker. */
	readonly short: string;
	/** Whether the last read failed, so the figures are older and draw dimmed. */
	readonly stale: boolean;
	/** When the figures were read, and why the last read failed, for a tooltip. */
	readonly title: string;
};

/** A bar that shows what's left of `total`, such as "$7.42 left of $10.00". */
const leftOf = (label: string, left: number, total: number): CreditsBar => ({
	window: {
		label,
		minutes: 0,
		percent: usedPercent(left, total),
		usedUp: left <= 0,
		resetsAt: null,
		blockedByWeek: false,
		spend: { used_cents: Math.max(0, total - left), limit_cents: total },
	},
	text: t("{{left}} left of {{total}}", { left: money(left), total: money(total) }),
});

/** When the key's limit starts over, such as "Resets monthly", or "" for one that never does. */
export const resetsText = (resets: string | null | undefined): string => {
	switch (resets ?? "") {
		case "":
			return "";
		case "daily":
			return t("Resets daily");
		case "weekly":
			return t("Resets weekly");
		case "monthly":
			return t("Resets monthly");
		default:
			return t("Resets: {{when}}", { when: resets });
	}
};

/** What the key spent today, such as "$0.31 today", or "" when OpenRouter didn't say. */
const todayText = (credits: ServerCredits): string =>
	credits.today_cents != null ? t("{{spent}} today", { spent: money(credits.today_cents) }) : "";

/** The parts that come second, joined, leaving out those with nothing to say. */
const joined = (...parts: string[]): string => parts.filter(Boolean).join(" · ");

/**
 * A server's credits as the meter shows them:
 * - with the account's balance: "$7.42 left of $10.00" from the account,
 *   and "This key: $2.58" second;
 * - with the key's limit: "$7.42 left of $10.00", and "Resets monthly";
 * - without either: "$2.58 spent", and "$0.31 today".
 */
export const creditsView = (credits: ServerCredits, now: Timestamp): CreditsView => {
	const stale = credits.error != null;
	const read = stale
		? t("{{error}} These figures are from {{when}}.", {
				error: credits.error,
				when: formatWhen(credits.at, now),
			})
		: t("Checked {{ago}}.", { ago: formatAgo(credits.at, now) });
	const { balance_cents: balance, bought_cents: bought, limit_cents: limit } = credits;
	const spent = money(credits.used_cents);
	if (balance != null && bought != null) {
		const bar = leftOf(t("Account"), balance, bought);
		const status = t("This key: {{spent}}", { spent });
		return {
			bars: [bar],
			status,
			line: joined(bar.text, status),
			short: t("{{left}} left", { left: money(balance) }),
			stale,
			title: read,
		};
	}
	if (limit != null) {
		const left = credits.remaining_cents ?? Math.max(0, limit - credits.used_cents);
		const bar = leftOf(t("This key"), left, limit);
		const status = joined(resetsText(credits.resets), todayText(credits));
		return {
			bars: [bar],
			status,
			line: joined(bar.text, status),
			short: t("{{left}} left", { left: money(left) }),
			stale,
			title: read,
		};
	}
	const text = t("{{spent}} spent", { spent });
	const status = todayText(credits);
	return {
		bars: [
			{
				window: {
					label: t("This key"),
					minutes: 0,
					percent: null,
					usedUp: false,
					resetsAt: null,
					blockedByWeek: false,
					spend: null,
				},
				text,
			},
		],
		status,
		line: joined(text, status),
		short: text,
		stale,
		title: read,
	};
};

/** A server whose credits show, with them ready to draw. */
export type ServerCreditsRow = {
	readonly server: ModelServer;
	readonly view: CreditsView;
};

/**
 * The servers with credits, for the usage popover's rows below the
 * vendors' accounts, by name. Servers without credits stay out.
 */
export const serverCreditRows = (view: ViewState, now: Timestamp): ServerCreditsRow[] =>
	all(view, "model_server")
		.filter((server) => server.credits != null)
		.sort((a, b) => a.label.localeCompare(b.label) || a.id.localeCompare(b.id))
		.map((server) => ({ server, view: creditsView(server.credits!, now) }));

/**
 * A server's main figure for the model picker, such as "$7.42 left" or
 * "$2.58 spent", or null without credits.
 */
export const creditsShort = (server: Pick<ModelServer, "credits">): string | null =>
	server.credits ? creditsView(server.credits, server.credits.at).short : null;

/**
 * Whether a server's row offers an admin key for the account's balance: an
 * OpenRouter server with a token, on a device whose `maxd` keeps one, from
 * `CLOUD_SERVERS_SINCE` on. A server that has one always shows it, with
 * Remove.
 */
export const offersAdminKey = (
	view: ViewState,
	device: DeviceId,
	server: Pick<ModelServer, "base_url" | "token" | "admin_key">
): boolean => {
	if (server.admin_key) return true;
	return (
		takesCloudServers(view, device) && server.token === true && isOpenRouterUrl(server.base_url)
	);
};

/** The action that keeps `key` as a server's admin key, trimmed. */
export const setAdminKeyAction = (server: ModelServerId, key: string): ModelAction => ({
	action: "set_server_admin_key",
	server,
	key: key.trim() || null,
});

/** The action that removes a server's admin key. */
export const removeAdminKeyAction = (server: ModelServerId): ModelAction => ({
	action: "set_server_admin_key",
	server,
	key: null,
});

/** What an admin key can do, said where it's added. */
export const adminKeyWarning = (): string =>
	t(
		"An admin key can create and delete keys on your OpenRouter account. MaxHQ keeps it on this device and uses it only to read your balance."
	);
