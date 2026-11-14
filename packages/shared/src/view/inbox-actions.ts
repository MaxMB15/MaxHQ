// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the inbox menus offer for one agent or several, what the notice
// says once a change goes through, how to undo it, and what a failure
// reads as. Each platform's sidebar draws its menus from these, so the
// menus offer the same things everywhere.

import { t } from "../i18n";

import type { Agent, AgentId, Inbox, InboxChange, Timestamp } from "../bindings";
import type { ActionResult } from "../data/source";
import { formatWhen, plural, sentence } from "../status/format";
import { canContinueAtLimit, continuesAfterLimit } from "../settings/limits";
import { atLimit, blockingWindow } from "../status/usage";

import { inboxOf, isSnoozed } from "./inbox";
import { nextPinKey, planPinnedReorder } from "./pin-order";
import { all, type ViewState } from "./replica";

/** One `setInbox` call: the agents and the change they all get. */
export type InboxStep = { readonly agents: readonly AgentId[]; readonly change: InboxChange };

/** A menu choice that changes some of the agents it was opened for. */
export type InboxChoice = {
	/** The words in the menu, such as "Settle" or "Settle (3)". */
	readonly label: string;
	/** The calls it makes, one for most choices. */
	readonly steps: readonly InboxStep[];
};

/** A label with the count after it when the menu acts on several agents. */
const counted = (label: string, count: number, several: boolean): string =>
	several ? `${label} (${count})` : label;

const ids = (agents: readonly Agent[]): AgentId[] => agents.map((agent) => agent.id);

const choice = (
	label: string,
	agents: readonly Agent[],
	change: InboxChange,
	several: boolean
): InboxChoice | null =>
	agents.length === 0
		? null
		: {
				label: counted(label, agents.length, several),
				steps: [{ agents: ids(agents), change }],
			};

/**
 * The agents a menu acts on, in the order given. A sub-agent goes where
 * its lead goes, so only leads count.
 */
export const inboxTargets = (view: ViewState, agents: Iterable<AgentId>): Agent[] =>
	[...agents].flatMap((id) => {
		const agent = view.tables.agent.get(id);
		return agent && agent.parent === null ? [agent] : [];
	});

/**
 * When the agent's limit frees up, if it's at the limit now: the reset it
 * waits for, or else the reset of its account's used-up window. A window
 * counts as used up at 99%. Null when there's no reset in the future.
 */
export const limitResetOf = (view: ViewState, agent: Agent, now: Timestamp): Timestamp | null => {
	const { activity } = agent;
	if (activity.state === "waiting_for_limit_reset" && activity.resets_at !== null) {
		return activity.resets_at > now ? activity.resets_at : null;
	}
	const account = agent.account ? view.tables.account.get(agent.account) : undefined;
	if (!account || !atLimit(account)) return null;
	const resets = blockingWindow(account)?.resetsAt ?? null;
	return resets !== null && resets > now ? resets : null;
};

/** What the menus offer for some agents. Each part is null when it doesn't apply. */
export type InboxMenu = {
	/** Settle the ones that aren't settled, or bring them all back. */
	readonly settle: InboxChoice | null;
	/** Pin the ones that aren't pinned, at the end, or unpin them all. */
	readonly pin: InboxChoice | null;
	/** Ends the snooze, when every one is snoozed. */
	readonly wake: InboxChoice | null;
	/** The agents a snooze applies to, when it's offered. */
	readonly snooze: readonly AgentId[] | null;
	/**
	 * "Until the limit resets", when every agent is at its limit. Agents
	 * on different accounts wake at different times, one call for each.
	 */
	readonly untilLimit: InboxChoice | null;
	/** Turns auto-settle on for the ones it's off for. */
	readonly autoSettleOn: InboxChoice | null;
	/** Turns auto-settle off for the ones it's on for. */
	readonly autoSettleOff: InboxChoice | null;
	/** Whether auto-settle is on for all of them, off for all, or mixed. */
	readonly autoSettle: "on" | "off" | "mixed";
	/**
	 * Has the ones that don't carry on after a usage limit carry on. Null
	 * unless every agent is a Claude Code or Codex agent MaxHQ holds, on a
	 * `maxd` that has the switch.
	 */
	readonly continueOn: InboxChoice | null;
	/** Stops the ones that carry on after a usage limit from doing so. */
	readonly continueOff: InboxChoice | null;
	/** Whether they all carry on after a limit, none do, or some do. */
	readonly continueAtLimit: "on" | "off" | "mixed";
};

/** The menu for these agents, which come from `inboxTargets`. */
export const inboxMenu = (
	view: ViewState,
	targets: readonly Agent[],
	now: Timestamp
): InboxMenu => {
	const several = targets.length > 1;
	const inboxes = targets.map((agent) => ({ agent, inbox: inboxOf(agent) }));
	const where = (test: (inbox: Inbox) => boolean) =>
		inboxes.filter(({ inbox }) => test(inbox)).map(({ agent }) => agent);

	const unsettled = where((inbox) => inbox.settled === null);
	const settle =
		unsettled.length > 0
			? choice(t("Settle"), unsettled, { change: "settle" }, several)
			: choice(t("Unsettle"), targets, { change: "unsettle" }, several);

	const unpinned = where((inbox) => inbox.pin === null);
	const pinKeys = all(view, "agent").flatMap((agent) => {
		const pin = inboxOf(agent).pin;
		return pin ? [pin.key] : [];
	});
	const pin =
		unpinned.length > 0
			? choice(t("Pin"), unpinned, { change: "pin", key: nextPinKey(pinKeys) }, several)
			: choice(t("Unpin"), targets, { change: "unpin" }, several);

	const snoozed = where((inbox) => isSnoozed(inbox, now));
	const allSnoozed = targets.length > 0 && snoozed.length === targets.length;

	const resets = targets.map((agent) => limitResetOf(view, agent, now));
	let untilLimit: InboxChoice | null = null;
	if (!allSnoozed && targets.length > 0 && resets.every((at) => at !== null)) {
		const byTime = new Map<Timestamp, AgentId[]>();
		targets.forEach((agent, i) => {
			const at = resets[i]!;
			byTime.set(at, [...(byTime.get(at) ?? []), agent.id]);
		});
		untilLimit = {
			label: t("Until the limit resets"),
			steps: [...byTime].map(([until, agents]) => ({
				agents,
				change: { change: "snooze", until },
			})),
		};
	}

	const off = where((inbox) => inbox.auto_settle_off);
	const on = where((inbox) => !inbox.auto_settle_off);

	const switchable =
		targets.length > 0 && targets.every((agent) => canContinueAtLimit(view, agent));
	const carrying = switchable ? targets.filter((agent) => continuesAfterLimit(view, agent)) : [];
	const stopping = switchable ? targets.filter((agent) => !continuesAfterLimit(view, agent)) : [];
	return {
		settle,
		pin,
		wake: allSnoozed ? choice(t("Wake"), targets, { change: "wake" }, several) : null,
		snooze: allSnoozed || targets.length === 0 ? null : ids(targets),
		untilLimit,
		autoSettleOn: choice(t("On"), off, { change: "auto_settle", on: true }, several),
		autoSettleOff: choice(t("Off"), on, { change: "auto_settle", on: false }, several),
		autoSettle: off.length === 0 ? "on" : on.length === 0 ? "off" : "mixed",
		continueOn: choice(t("On"), stopping, { change: "continue_at_limit", on: true }, several),
		continueOff: choice(
			t("Off"),
			carrying,
			{ change: "continue_at_limit", on: false },
			several
		),
		continueAtLimit: stopping.length === 0 ? "on" : carrying.length === 0 ? "off" : "mixed",
	};
};

/**
 * The calls that move a pinned agent to `slot` among the pins shown, where
 * a slot is the gap before the pin at that index and the last slot is
 * after them all. Usually one call; when its neighbors leave no room, every
 * shown pin takes a new key. None when it lands where it was.
 */
export const pinMoveSteps = (
	view: ViewState,
	shown: readonly AgentId[],
	moved: AgentId,
	slot: number
): InboxStep[] => {
	const from = shown.indexOf(moved);
	if (from === -1 || slot === from || slot === from + 1) return [];
	const orderedIds = shown.filter((id) => id !== moved);
	orderedIds.splice(slot > from ? slot - 1 : slot, 0, moved);
	const keysById = new Map(
		all(view, "agent").flatMap((agent) => {
			const pin = inboxOf(agent).pin;
			return pin ? [[agent.id, pin.key] as const] : [];
		})
	);
	return planPinnedReorder({ orderedIds, keysById, movedId: moved }).map(({ id, key }) => ({
		agents: [id],
		change: { change: "pin", key },
	}));
};

/** The agents named in a sentence: the title for one, a count for several. */
const who = (agents: readonly Agent[]): string =>
	agents.length === 1 ? agents[0]!.title : plural(agents.length, "agent");

/**
 * What the notice says once a change went through for these agents, as
 * they were before it. Pinning agents that were all pinned already is a
 * move. Null for a change that leaves no notice.
 */
export const doneText = (
	change: InboxChange,
	agents: readonly Agent[],
	now: Timestamp
): string | null => {
	if (agents.length === 0) return null;
	const them = who(agents);
	switch (change.change) {
		case "settle":
			return t("Settled {{them}}", { them: them });
		case "unsettle":
			return t("Unsettled {{them}}", { them: them });
		case "snooze":
			return t("Snoozed {{them}} until {{formatWhen_change_until__now}}", {
				them: them,
				formatWhen_change_until__now: formatWhen(change.until, now),
			});
		case "wake":
			return t("Woke {{them}}", { them: them });
		case "pin":
			return agents.every((agent) => inboxOf(agent).pin !== null)
				? t("Moved {{them}}", { them: them })
				: t("Pinned {{them}}", { them: them });
		case "unpin":
			return t("Unpinned {{them}}", { them: them });
		case "auto_settle":
			return t("Auto-settle {{change_on____on_____off}} for {{them}}", {
				change_on____on_____off: change.on ? t("on") : t("off"),
				them: them,
			});
		case "continue_at_limit":
			return t("Continue after the limit {{change_on____on_____off}} for {{them}}", {
				change_on____on_____off: change.on ? t("on") : t("off"),
				them: them,
			});
		case "dismiss_woke":
		case "restore":
			return null;
	}
};

/**
 * What the notice says once agents snoozed until their limits reset, which
 * can be a different time for each account.
 */
export const limitSnoozedText = (agents: readonly Agent[]): string =>
	t("Snoozed {{who_agents}} until the limit resets", { who_agents: who(agents) });

/** The words for agents that auto-settle just settled. */
export const autoSettledText = (count: number): string =>
	t("Auto-settled {{plural_count___idle_agent}}", {
		plural_count___idle_agent: plural(count, "idle agent"),
	});

/**
 * The calls that put these agents back as they were. `restore` gives every
 * agent in a call the same inbox, so agents that differed take a call each.
 */
export const restoreSteps = (before: readonly Agent[]): InboxStep[] => {
	const groups = new Map<string, { inbox: Inbox; agents: AgentId[] }>();
	for (const agent of before) {
		const inbox = inboxOf(agent);
		const key = JSON.stringify(inbox);
		const group = groups.get(key) ?? { inbox, agents: [] };
		group.agents.push(agent.id);
		groups.set(key, group);
	}
	return [...groups.values()].map(({ inbox, agents }) => ({
		agents,
		change: { change: "restore", inbox },
	}));
};

/**
 * Agents auto-settle settled between two views: settled by it now, and in
 * the earlier view without that. An agent the earlier view didn't have
 * doesn't count, since it's new to the window rather than just settled.
 */
export const newlyAutoSettled = (before: ViewState, after: ViewState): Agent[] => {
	if (before.tables.agent === after.tables.agent) return [];
	return all(after, "agent").filter((agent) => {
		if (agent.parent !== null || inboxOf(agent).settled?.by !== "auto") return false;
		const was = before.tables.agent.get(agent.id);
		return was !== undefined && inboxOf(was).settled?.by !== "auto";
	});
};

/** Why one agent's change didn't go through, or null when it did. */
export const inboxFailureOf = (result: ActionResult): string | null => {
	if (result.status === "error") {
		const { error } = result;
		switch (error.kind) {
			case "not_found":
				return t("{{sentence_error_what}} isn't there anymore", {
					sentence_error_what: sentence(error.what),
				});
			case "not_connected":
				return t("{{error_device}} isn't connected", { error_device: error.device });
			case "failed":
				return sentence(error.message);
		}
	}
	switch (result.data.status) {
		case "refused":
			return sentence(result.data.reason);
		case "unknown_agent":
			return t("MaxHQ no longer knows this agent");
		default:
			return null;
	}
};

const TRIED: Record<InboxChange["change"], string> = {
	settle: "settle",
	unsettle: "unsettle",
	snooze: "snooze",
	wake: "wake",
	get dismiss_woke() {
		return t("clear the marker on");
	},
	pin: "pin",
	unpin: "unpin",
	get auto_settle() {
		return t("change auto-settle for");
	},
	get continue_at_limit() {
		return t("change continuing after the limit for");
	},
	get restore() {
		return t("undo the change to");
	},
};

/** An agent something failed for, and why. */
export type Failed = { readonly agent: Agent; readonly reason: string };

/**
 * One sentence for each reason the change failed, naming the agents it
 * failed for, such as "Couldn't settle 2 agents. Devbox isn't connected."
 */
export const failureLines = (change: InboxChange, failed: readonly Failed[]): string[] =>
	triedLines(TRIED[change.change], failed);

/**
 * One sentence for each reason what you tried failed, such as "delete",
 * naming the agents it failed for.
 */
export const triedLines = (tried: string, failed: readonly Failed[]): string[] => {
	const byReason = new Map<string, Agent[]>();
	for (const { agent, reason } of failed) {
		byReason.set(reason, [...(byReason.get(reason) ?? []), agent]);
	}
	return [...byReason].map(([reason, agents]) => {
		const words = t("Couldn't {{tried}} {{who_agents}}", {
			tried: tried,
			who_agents: who(agents),
		});
		return reason === "" ? `${words}.` : `${words}. ${reason}.`;
	});
};
