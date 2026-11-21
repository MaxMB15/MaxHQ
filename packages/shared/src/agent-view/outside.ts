// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A session someone started in a terminal. While the terminal runs it,
// MaxHQ only follows along, until you take it over: MaxHQ then stops the
// CLI there, now or once its turn ends. Once it ends, the session can
// continue here.
// A sub-agent runs inside its lead's CLI, so taking it over takes over the
// lead at the top of its tree. Once that lead continues in MaxHQ, the
// sub-agent's view points there.
// While MaxHQ can't reach the device, it says so instead. The desktop and
// the phone say the same.

import { t } from "../i18n";

import { useCallback, useEffect, useState } from "react";

import {
	type Agent,
	type AgentId,
	type LinkState,
	TAKE_OVER_SINCE,
	type TakeOverMode,
	type Timestamp,
	type Vendor,
} from "../bindings";
import type { DataSource } from "../data/source";
import { errorReason, isWorking } from "../status/activity";
import { formatAgo, formatWhen } from "../status/format";
import { kindName, ownKind } from "../status/agent-kind";
import { failureOf, type SendState, useSend } from "./answer";
import { keep, keptValue } from "./kept";

/** The command each vendor's CLI runs as. MaxHQ knows none for another ACP agent. */
export const CLI: Record<Vendor, string> = {
	claude: "claude",
	codex: "codex",
	cursor: "agent",
	acp: "",
};

/** A run of a note's text, set as code when it's something to type. */
export type NotePart = { readonly text: string; readonly code?: true };

/** What the banner says in place of the composer. */
export type OutsideNote = {
	/** The terminal let go, so the session can continue in MaxHQ. */
	readonly ended: boolean;
	readonly title: string;
	readonly body: readonly NotePart[];
	/** Why Continue in MaxHQ is off, while it is. */
	readonly wait: readonly NotePart[];
};

/** What the view knows of the device's link, and the moment to describe. */
export type OutsideReach = {
	/** The device's link state, or null when the view has no link for it. */
	readonly link: LinkState | null;
	readonly now: Timestamp;
};

/** Why Continue in MaxHQ waits while the terminal runs the session. */
export const WAIT_NOTE: readonly NotePart[] = [
	{
		get text() {
			return t("Turns on when the session in the terminal ends. Quit it there with ");
		},
	},
	{ text: "/exit", code: true },
	{
		get text() {
			return t(". A session can only run in one place at a time.");
		},
	},
];

/** The way to let go of the session by hand, under Take over. */
export const MANUAL_NOTE: readonly NotePart[] = [
	{
		get text() {
			return t("Or quit it in the terminal yourself with ");
		},
	},
	{ text: "/exit", code: true },
	{
		get text() {
			return t(". A session can only run in one place at a time.");
		},
	},
];

const FOLLOWS =
	" in a terminal. Answer it and send messages in that terminal. This view follows along.";

/** "It last answered at 14:36, 12 min ago. ", when the view knows. */
const lastAnswered = (reach: OutsideReach | undefined): string => {
	if (reach?.link?.state !== "unreachable" || reach.link.last_seen === null) return "";
	const seen = reach.link.last_seen;
	return t(
		"It last answered at {{formatWhen_seen__reach_now}}, {{formatAgo_seen__reach_now}}. ",
		{
			formatWhen_seen__reach_now: formatWhen(seen, reach.now),
			formatAgo_seen__reach_now: formatAgo(seen, reach.now),
		}
	);
};

/**
 * The note for `agent`, started in a terminal on `place` when that's known.
 * Pass `reach` to say when the device last answered while it can't be reached.
 */
export const outsideNote = (
	agent: Agent,
	place: string | null,
	reach?: OutsideReach
): OutsideNote => {
	const error = agent.activity.state === "error" ? `${errorReason(agent.activity.reason)} ` : "";
	if (reach?.link?.state === "unreachable" || agent.activity.state === "not_accessible") {
		const device = place ?? t("the device");
		return {
			ended: false,
			title: t("Can't reach {{device}} right now.", { device: device }),
			body: [
				{
					text: t(
						"{{error}}{{lastAnswered_reach}}The session may still run in the terminal there. This view shows what MaxHQ saw last.",
						{ error: error, lastAnswered_reach: lastAnswered(reach) }
					),
				},
			],
			wait: [
				{
					text: t(
						"Turns on once MaxHQ reaches {{device}} again and the session in the terminal has ended.",
						{ device: device }
					),
				},
			],
		};
	}
	if (agent.link.state === "down") {
		return {
			ended: true,
			title: error
				? t("The terminal session stopped with an error. You can continue it here.")
				: t("The terminal session ended. You can continue it here."),
			body: [
				{
					text: t(
						"{{error}}{{VENDOR_NAME_agent_vendor}} keeps the history and context. The terminal is no longer attached.",
						{ error: error, VENDOR_NAME_agent_vendor: kindName(ownKind(agent)) }
					),
				},
			],
			wait: WAIT_NOTE,
		};
	}
	const cli = CLI[agent.vendor];
	const on = place ? t(" on {{value1}}", { value1: place }) : "";
	const started: NotePart[] = cli
		? [{ text: t("{{error}}Started with ", { error: error }) }, { text: cli, code: true }]
		: [{ text: t("{{error}}Started", { error: error }) }];
	return {
		ended: false,
		title: error
			? t("Stopped with an error in a terminal{{on}}. Read only here.", { on: on })
			: t("Running in a terminal{{on}}. Read only here.", { on: on }),
		body: [...started, { text: error ? t(" in a terminal.") : FOLLOWS }],
		wait: WAIT_NOTE,
	};
};

/** Continues `agent` in MaxHQ, and tracks how it went. */
export const useContinueHere = (source: DataSource, agent: AgentId): [SendState, () => void] => {
	const [state, setState] = useState<SendState>({ state: "idle" });
	const go = useCallback(() => {
		setState({ state: "sending" });
		source.continueHere(agent).then(
			(result) => {
				const failure = failureOf(result);
				setState(failure ? { state: "failed", message: failure } : { state: "sent" });
			},
			(error: unknown) =>
				setState({
					state: "failed",
					message:
						error instanceof Error
							? error.message
							: t("Couldn't continue the session here."),
				})
		);
	}, [source, agent]);
	return [state, go];
};

/**
 * Whether MaxHQ can stop the CLI in the terminal to take `agent` over: a
 * Claude or Codex session running there, on a reachable `maxd` whose
 * `protocol` knows how. Null when the view has no link for it.
 */
export const canTakeOver = (
	agent: Agent,
	protocol: number | null,
	link: LinkState | null
): boolean =>
	agent.control === "observed" &&
	agent.parent === null &&
	agent.link.state === "live" &&
	agent.activity.state !== "not_accessible" &&
	link?.state !== "unreachable" &&
	(agent.vendor === "claude" || agent.vendor === "codex") &&
	protocol !== null &&
	protocol >= TAKE_OVER_SINCE;

// How far up a sub-agent's parents `rootLead` looks. maxd nests three deep.
const DEPTH = 8;

/**
 * The lead at the top of `agent`'s tree: `agent` itself when it leads, or
 * null when the view lacks one of its parents.
 */
export const rootLead = (agents: ReadonlyMap<string, Agent>, agent: Agent): Agent | null => {
	let at = agent;
	for (let depth = 0; at.parent !== null; depth++) {
		const parent = agents.get(at.parent);
		if (!parent || depth === DEPTH) return null;
		at = parent;
	}
	return at;
};

/** What the banner offers in place of the composer. */
export type OutsideOffer =
	/** Take over, which asks first. */
	| { readonly kind: "take_over" }
	/** Cancel, while MaxHQ waits for the turn to end to take over. */
	| { readonly kind: "taking" }
	/** Continue in MaxHQ, once the terminal let go. */
	| { readonly kind: "continue" }
	/** Open session: a sub-agent's session continues in MaxHQ as `lead`. */
	| { readonly kind: "moved"; readonly lead: Agent }
	/** `button` stays off, and `why` says why. */
	| {
			readonly kind: "off";
			readonly button: "take_over" | "continue";
			readonly why: readonly NotePart[];
	  };

const off = (text: string): OutsideOffer => ({ kind: "off", button: "take_over", why: [{ text }] });

/**
 * What the banner offers for `agent`, given its `note`, the `lead` at the
 * top of its tree (null when not in view), and the `protocol` and `link` of
 * the device it runs on, called `place`. A sub-agent offers to take over
 * its lead, and says why not when it can't, since quitting the CLI is up to
 * the lead.
 */
export const outsideOffer = (
	agent: Agent,
	lead: Agent | null,
	note: OutsideNote,
	protocol: number | null,
	link: LinkState | null,
	place: string | null
): OutsideOffer => {
	if (agent.parent === null) {
		if (note.ended) return { kind: "continue" };
		if (agent.taking_over) return { kind: "taking" };
		if (canTakeOver(agent, protocol, link)) return { kind: "take_over" };
		return { kind: "off", button: "continue", why: note.wait };
	}
	if (!lead)
		return off(
			t("MaxHQ can't see the session this sub-agent runs in, so it can't take it over.")
		);
	if (lead.control === "held") return { kind: "moved", lead };
	if (lead.taking_over) return { kind: "taking" };
	if (canTakeOver(lead, protocol, link)) return { kind: "take_over" };
	const device = place ?? t("the device");
	if (link?.state === "unreachable" || lead.activity.state === "not_accessible")
		return off(t("Turns on once MaxHQ reaches {{device}} again.", { device }));
	if (lead.link.state === "down")
		return off(
			t(
				"The session this sub-agent ran in left the terminal. Continue it here from its lead."
			)
		);
	if (lead.vendor !== "claude" && lead.vendor !== "codex")
		return off(t("MaxHQ can take over only Claude and Codex sessions from a terminal."));
	return off(t("Taking over needs a newer maxd on {{device}}.", { device }));
};

const LEAD = "outside-lead:";

/**
 * Remembers that the sub-agent `agent` runs in the session `lead` leads, so
 * its view can follow the session there once the sub-agent is gone.
 */
export const followLead = (agent: AgentId, lead: AgentId) => {
	if (lead !== agent) keep(LEAD + agent, lead);
};

/** Remembers the lead of a sub-agent while its view shows, as `followLead` does. */
export const useFollowLead = (agent: AgentId, lead: AgentId | null) => {
	useEffect(() => {
		if (lead !== null) followLead(agent, lead);
	}, [agent, lead]);
};

/**
 * The lead whose session the sub-agent `agent` ran in, once it continues in
 * MaxHQ. Only a sub-agent whose view showed in this window knows it.
 */
export const movedTo = (agents: ReadonlyMap<string, Agent>, agent: AgentId): Agent | null => {
	const lead = keptValue<AgentId | null>(LEAD + agent, null);
	const found = lead === null ? undefined : agents.get(lead);
	return found?.control === "held" ? found : null;
};

/** What a sub-agent's view says once its session continues in MaxHQ as `lead`. */
export const movedNote = (lead: Agent): { readonly title: string; readonly body: string } => ({
	title: t("This sub-agent's session continues in MaxHQ."),
	body: t(
		"The sub-agent stopped when its session left the terminal. {{vendor}} keeps its history, so the session can pick it up again, and then it shows here.",
		{ vendor: kindName(ownKind(lead)) }
	),
});

/** Whether a turn runs, so a take-over can wait for it to end. */
export const inTurn = (agent: Agent): boolean =>
	isWorking(agent.activity) || agent.activity.state === "needs_input";

/** What MaxHQ asks before it takes a session over. */
export type TakeOverQuestion = {
	readonly title: string;
	/** The risk: what stops, and what stays. */
	readonly detail: string;
	/** Takes over after the turn while one runs, or at once. */
	readonly action: string;
	/** Stops at once, offered only while a turn runs. */
	readonly alternative?: string;
};

/**
 * The question before MaxHQ takes over `lead`, run in a terminal on
 * `device`, asked from the view of `agent`: the lead itself or a sub-agent
 * in its tree. `subAgents` says whether the lead has any, which stop with it.
 */
export const takeOverQuestion = (
	agent: Agent,
	device: string,
	lead: Agent = agent,
	subAgents = false
): TakeOverQuestion => {
	const cli = CLI[lead.vendor];
	const vendor = kindName(ownKind(lead));
	const own = agent.id === lead.id;
	const title = own ? t("Take over this session?") : t("Take over this sub-agent's session?");
	const detail = [
		own
			? t(
					"This stops {{cli}} in the terminal on {{device}}. The terminal window stays open, but the session leaves it and continues here.",
					{ cli, device }
				)
			: t(
					"This sub-agent runs inside a {{vendor}} session in the terminal on {{device}}. Taking over stops {{cli}} there and continues that session here.",
					{ vendor, cli, device }
				),
	];
	if (subAgents || !own)
		detail.push(
			t(
				"Sub-agents still working stop with it. {{vendor}} keeps their history, so the session can pick them up again once it runs here.",
				{ vendor }
			)
		);
	if (!inTurn(lead)) return { title, detail: detail.join(" "), action: t("Take over") };
	detail.push(t("Stop now cuts off the turn in progress."));
	return {
		title,
		detail: detail.join(" "),
		action: t("Take over after this turn"),
		alternative: t("Stop now"),
	};
};

/**
 * What the banner says while MaxHQ waits for the turn to end, on `place`
 * when known, in the view of `agent` or of a sub-agent of `lead`.
 */
export const takingOverNote = (agent: Agent, place: string | null, lead: Agent = agent): string => {
	const on = place ? t(" on {{value1}}", { value1: place }) : "";
	const cli = CLI[lead.vendor];
	return agent.id === lead.id
		? t(
				"MaxHQ stops {{cli}} in the terminal{{on}} once this turn ends, then continues the session here.",
				{ cli, on }
			)
		: t(
				"MaxHQ stops {{cli}} in the terminal{{on}} once the session's turn ends, then continues the session here.",
				{ cli, on }
			);
};

/** Takes `agent` over from its terminal, and tracks how the last request went. */
export const useTakeOver = (
	source: DataSource,
	agent: AgentId
): [SendState, (mode: TakeOverMode) => void] =>
	useSend<TakeOverMode>(useCallback((mode) => source.takeOver(agent, mode), [source, agent]));
