// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A Codex account's banked rate-limit resets. The usage popover and the
// phone's Usage card say how many are banked under the account. On the
// desktop, Use reset spends one, which can't be undone, so it asks first
// and shows only where the account's device has a maxd that knows the
// request. Codex makes the request with its own sign-in on that device.

import { createStore } from "zustand/vanilla";

import { t } from "../i18n";

import { RESET_CREDITS_SINCE, type Account, type AccountId, type ActionOutcome } from "../bindings";
import type { Ask, Result } from "../data/request-source";
import { errorText } from "../timeline/store";
import type { ViewState } from "../view/replica";
import { linkForDevice } from "../view/select";

import { sentence } from "./format";

/** The banked reset calls of a data source. */
export type ResetCreditCalls = {
	/**
	 * Spends one of a Codex account's banked resets on the device whose
	 * login it is. Only the desktop app may: every other client is refused.
	 */
	readonly useResetCredit: (account: AccountId) => Promise<Result<ActionOutcome>>;
};

/** The banked reset calls, each one request over `ask`. */
export const resetCreditCalls = (ask: Ask): ResetCreditCalls => ({
	useResetCredit: (account) => ask({ kind: "use_reset_credit", account }),
});

/** How many resets a Codex account has banked, or null when nothing said. */
export const resetCredits = (account: Account): number | null =>
	account.vendor === "codex" ? (account.usage?.reset_credits ?? null) : null;

/** The quiet line under a Codex account, such as "2 resets banked", or none at 0. */
export const resetCreditsLine = (account: Account): string | undefined => {
	const count = resetCredits(account);
	if (count === null || count <= 0) return undefined;
	return t("{{count, number}} reset banked", { count });
};

/**
 * Whether the account shows Use reset: a Codex login in its own folder with
 * a reset banked, on a device whose maxd can spend one.
 */
export const canUseReset = (view: ViewState, account: Account): boolean => {
	const count = resetCredits(account);
	if (count === null || count <= 0) return false;
	if ((account.login ?? "config_dir") !== "config_dir") return false;
	const protocol = linkForDevice(view, account.device)?.protocol ?? null;
	return protocol !== null && protocol >= RESET_CREDITS_SINCE;
};

/** What the confirm asks before a reset is spent. */
export const resetQuestion = (
	count: number
): { readonly title: string; readonly detail: string; readonly action: string } => ({
	title: t("Use a Codex reset?"),
	detail: t(
		"This spends 1 of your {{count, number}} banked resets and resets this account's Codex limits now. It can't be undone.",
		{ count }
	),
	action: t("Use reset"),
});

/** What the button says while a reset is being spent. */
export const resetRunning = (): string => t("Using a reset…");

/** The line a spend leaves, and how it reads. */
export type ResetLine = {
	readonly text: string;
	/** `done` once a reset was spent, `quiet` when none was, `failed` otherwise. */
	readonly tone: "done" | "quiet" | "failed";
};

/** The line a spend's answer leaves under the account. */
export const resetLine = (result: Result<ActionOutcome>): ResetLine => {
	const failed = (reason: string): ResetLine => ({
		text: t("Couldn't use a reset. {{reason}}.", { reason: sentence(reason) }),
		tone: "failed",
	});
	if (result.status === "error") return failed(errorText(result.error));
	const outcome = result.data;
	if (outcome.status === "refused") return failed(outcome.reason);
	if (outcome.status !== "reset_credit") {
		return {
			text: t(
				"The device gave an answer MaxHQ doesn't know. Check the count before trying again."
			),
			tone: "failed",
		};
	}
	switch (outcome.outcome) {
		// The same click already went through, so the reset is spent.
		case "reset":
		case "already_redeemed":
			return {
				text: t("Reset used. This account's Codex limits start over now."),
				tone: "done",
			};
		case "nothing_to_reset":
			return {
				text: t("No limit needs a reset right now, so nothing was spent."),
				tone: "quiet",
			};
		case "no_credit":
			return { text: t("No resets left."), tone: "quiet" };
	}
};

/** Where an account's spend stands: still out, or the line its answer left. */
export type ResetSpend =
	{ readonly state: "running" } | { readonly state: "done"; readonly line: ResetLine };

/** Each account's latest spend. */
export type ResetSpends = { readonly spends: ReadonlyMap<AccountId, ResetSpend> };

/**
 * The spends of one window, kept apart from the view that shows them so a
 * line outlives the popover closing. A second press while one is out sends
 * nothing.
 */
export const resetSpends = (calls: ResetCreditCalls) => {
	const store = createStore<ResetSpends>(() => ({ spends: new Map() }));
	const put = (account: AccountId, spend: ResetSpend) =>
		store.setState((s) => ({ spends: new Map(s.spends).set(account, spend) }));
	return {
		store,
		/** Whether a spend for the account is out now. */
		running: (account: AccountId): boolean =>
			store.getState().spends.get(account)?.state === "running",
		/** Spends one of the account's resets, unless one is already out. */
		spend: async (account: AccountId): Promise<void> => {
			if (store.getState().spends.get(account)?.state === "running") return;
			put(account, { state: "running" });
			let result: Result<ActionOutcome>;
			try {
				result = await calls.useResetCredit(account);
			} catch (error) {
				result = { status: "error", error: { kind: "failed", message: String(error) } };
			}
			put(account, { state: "done", line: resetLine(result) });
		},
	};
};
