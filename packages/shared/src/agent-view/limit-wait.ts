// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a wait for a usage limit says, and stopping the agent carrying on by
// itself. Waiting messages resume it, and so does its switch for continuing
// after the limit. Taking the messages back clears the wait's auto resume,
// and turning the switch off stops the message maxd would send at the reset.

import { t } from "../i18n";

import type { Account, AgentId, TimelineItem, Timestamp } from "../bindings";
import type { ActionResult, DataSource } from "../data/source";
import { formatClock, formatDuration } from "../status/format";
import { accountName, blockingWindow, windowNoun } from "../status/usage";
import { failureOf } from "./answer";

/** What a wait that has ended says. */
export const limitWaitEnded = (took: number | null): string =>
	took !== null
		? t("Waited {{formatDuration_took}} for the usage limit to reset", {
				formatDuration_took: formatDuration(took),
			})
		: t("Waited for the usage limit to reset");

/** The heading of a wait still going on. */
export const limitWaitTitle = (resets: Timestamp | null): string =>
	resets !== null
		? t("Waiting for usage limit reset, resumes {{formatClock_resets}}", {
				formatClock_resets: formatClock(resets),
			})
		: t("Waiting for usage limit reset");

/** What a wait still going on says happens next. */
export const limitWaitBody = (
	account: Account | undefined,
	resumes: boolean,
	resets: Timestamp | null,
	now: Timestamp
): string => {
	const name = account ? accountName(account) : t("The account");
	const blocking = account ? blockingWindow(account) : undefined;
	const limit = blocking
		? t("its {{value1}} limit", { value1: windowNoun(blocking.minutes).replace(" window", "") })
		: t("its limit");
	if (!resumes)
		return t("{{name}} is at {{limit}}. Send a message once it resets to carry on.", {
			name: name,
			limit: limit,
		});
	return resets !== null
		? t(
				"{{name}} is at {{limit}}. This agent continues on its own in {{formatDuration_Math_max_0__resets___now}}, and nothing is lost meanwhile.",
				{
					name: name,
					limit: limit,
					formatDuration_Math_max_0__resets___now: formatDuration(
						Math.max(0, resets - now)
					),
				}
			)
		: t(
				"{{name}} is at {{limit}}. This agent continues on its own once it resets, and nothing is lost meanwhile.",
				{ name: name, limit: limit }
			);
};

/** Why an action failed, or null when it went through. */
export const attempt = async (action: () => Promise<ActionResult>): Promise<string | null> => {
	try {
		return failureOf(await action());
	} catch (error) {
		return error instanceof Error && error.message ? error.message : t("It didn't go through.");
	}
};

/** Takes back the waiting messages one at a time, and says why one stayed. */
export const takeBack = async (
	cancel: (item: string) => Promise<ActionResult>,
	waiting: readonly TimelineItem[]
): Promise<string | null> => {
	for (const message of waiting) {
		const failure = await attempt(() => cancel(message.id));
		if (failure) return failure;
	}
	return null;
};

/**
 * Stops the agent carrying on by itself once the limit resets: turns its
 * switch for continuing after the limit off when `carriesOn`, then takes back
 * the waiting messages. Says why something stayed.
 */
export const stopResume = async (
	source: Pick<DataSource, "cancelQueued" | "setInbox">,
	agent: AgentId,
	waiting: readonly TimelineItem[],
	carriesOn: boolean
): Promise<string | null> => {
	if (carriesOn) {
		const failure = await attempt(async () => {
			const [result] = await source.setInbox([agent], {
				change: "continue_at_limit",
				on: false,
			});
			if (!result) throw new Error("It didn't go through.");
			return result;
		});
		if (failure)
			return t("Couldn't turn off continuing after the limit. {{failure}}", {
				failure: failure,
			});
	}
	const kept = await takeBack((item) => source.cancelQueued(agent, item), waiting);
	return kept ? t("Couldn't take back the waiting messages. {{kept}}", { kept: kept }) : null;
};
