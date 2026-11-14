// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The inbox rules, as crates/core/src/inbox.rs has them: what a change does
// to an agent's inbox, what brings an agent back by itself, and when
// auto-settle settles one. The sample data applies changes with these, and
// the sidebar reads snoozes with them, so keep the two in step.

import type { Activity, Agent, Inbox, InboxChange, Settled, Timestamp } from "../bindings";

const DAY_MS = 24 * 60 * 60 * 1000;

/** Where an agent sits before anything has moved it: in the active list. */
export const DEFAULT_INBOX: Inbox = {
	settled: null,
	kept_active: false,
	auto_settle_off: false,
	snoozed_until: null,
	snoozed_at: null,
	woke_at: null,
	pin: null,
};

/** The agent's inbox. An agent from a `maxd` before inboxes reads as active. */
export const inboxOf = (agent: Agent): Inbox => agent.inbox ?? DEFAULT_INBOX;

/**
 * Settles an inbox, which unpins it and ends its snooze. An inbox that is
 * already settled keeps its first settle.
 */
export const settle = (inbox: Inbox, settled: Settled): Inbox => ({
	...inbox,
	settled: inbox.settled ?? settled,
	kept_active: false,
	pin: null,
	snoozed_until: null,
	snoozed_at: null,
});

/** Ends a snooze and leaves the woke marker. An inbox that isn't snoozed stays as it is. */
export const wake = (inbox: Inbox, now: Timestamp): Inbox =>
	inbox.snoozed_until === null
		? inbox
		: { ...inbox, snoozed_until: null, snoozed_at: null, woke_at: now };

/**
 * A message you send brings the agent back: it unsettles it, ends its
 * snooze without a marker, and lets auto-settle watch it again.
 */
export const onMessage = (inbox: Inbox): Inbox => ({
	...inbox,
	settled: null,
	kept_active: false,
	snoozed_until: null,
	snoozed_at: null,
});

/**
 * Whether the inbox is snoozed now. A snooze whose time has passed has
 * ended, even before `maxd` clears it.
 */
export const isSnoozed = (inbox: Inbox, now: Timestamp): boolean =>
	inbox.snoozed_until !== null && inbox.snoozed_until > now;

/**
 * The inbox after a change. It doesn't check whether the agent may settle
 * or snooze: `maxd` refuses those before it applies the change.
 */
export const apply = (inbox: Inbox, change: InboxChange, now: Timestamp): Inbox => {
	switch (change.change) {
		case "settle":
			return settle(inbox, { at: now, by: "you" });
		case "unsettle":
			return { ...inbox, settled: null, kept_active: true };
		case "snooze":
			return { ...inbox, snoozed_until: change.until, snoozed_at: now };
		case "wake":
			return wake(inbox, now);
		case "dismiss_woke":
			return { ...inbox, woke_at: null };
		case "pin": {
			const unsettled =
				inbox.settled === null ? inbox : { ...inbox, settled: null, kept_active: true };
			const woke = wake(unsettled, now);
			return { ...woke, pin: { at: inbox.pin?.at ?? now, key: change.key } };
		}
		case "unpin":
			return { ...inbox, pin: null };
		case "auto_settle":
			return { ...inbox, auto_settle_off: !change.on };
		case "continue_at_limit":
			return { ...inbox, continue_at_limit: change.on };
		case "restore":
			return isSnoozed(change.inbox, now)
				? { ...change.inbox }
				: { ...change.inbox, snoozed_until: null, snoozed_at: null };
	}
};

/**
 * Whether an agent that turns to this activity wakes from its snooze
 * early: it needs you, or it failed. A finished turn doesn't wake it.
 */
export const wakesEarly = (activity: Activity): boolean =>
	activity.state === "needs_input" ||
	activity.state === "error" ||
	(activity.state === "paused" && activity.reason === "resumes_made_no_progress");

/**
 * Whether auto-settle settles a lead agent now. `days` is the setting that
 * applies to it, 0 for never. `unsaved` says whether its worktree has
 * changes or commits that aren't on the remote; not knowing counts as yes,
 * so auto-settle never settles work you'd lose sight of.
 */
export const autoSettles = (
	agent: Agent,
	days: number,
	unsaved: boolean | null,
	now: Timestamp
): boolean => {
	const inbox = inboxOf(agent);
	const { activity } = agent;
	const resting =
		activity.state === "idle" ||
		activity.state === "idle_cache_warm" ||
		activity.state === "not_accessible" ||
		(activity.state === "paused" && activity.reason !== "resumes_made_no_progress");
	const idleFor = Math.max(0, now - agent.last_activity_at);
	return (
		agent.parent === null &&
		days > 0 &&
		resting &&
		inbox.settled === null &&
		inbox.pin === null &&
		!inbox.kept_active &&
		!inbox.auto_settle_off &&
		!isSnoozed(inbox, now) &&
		idleFor >= days * DAY_MS &&
		unsaved === false
	);
};

/**
 * Auto-settle's record: settled by it, dated to the agent's last activity
 * so the settled list sorts by when the work stopped.
 */
export const autoSettled = (agent: Agent): Settled => ({
	at: agent.last_activity_at,
	by: "auto",
});
