// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The banner over the composer once an agent MaxHQ holds has stopped: you
// stopped it, it kept making no progress, or its CLI exited. Resume starts
// it again on the same conversation, and so does sending it a message. A
// local ACP agent that needs a sign-in gets the sign-in card instead, with
// the ways it says it signs in, and a local agent that can't load a session
// warns that its history doesn't come back. The desktop and the phone say
// the same.

import { t } from "../i18n";

import type { AcpAuthMethod, Account, Agent } from "../bindings";
import { historyWarning, keepsHistory, needsSignIn } from "../settings/acp-agents";
import { sentence } from "../status/format";

/** Whether an agent MaxHQ holds has stopped, so Resume can start it again. */
export const canResume = (agent: Agent): boolean => {
	if (agent.control === "observed" || agent.parent !== null) return false;
	if (agent.activity.state === "paused") return true;
	// On Resume, maxd starts a Claude CLI that exited. It doesn't do that
	// for a Codex agent that failed, so that one gets no banner.
	return (
		agent.activity.state === "error" && agent.vendor === "claude" && agent.link.state === "down"
	);
};

/** What the banner says, and whether it's the sign-in card. */
export type ResumeWording = {
	readonly signIn: boolean;
	readonly title: string;
	readonly body: string;
	/** The ways a local agent says it signs in, on the sign-in card. */
	readonly methods: readonly AcpAuthMethod[];
};

const stopWording = (agent: Agent): { title: string; body: string } => {
	const { activity } = agent;
	if (activity.state === "error") {
		return {
			title: t("Stopped with an error"),
			body: t("{{reason}}. The conversation stays, and Resume starts it again.", {
				reason: sentence(activity.reason),
			}),
		};
	}
	if (activity.state === "paused" && activity.reason === "resumes_made_no_progress") {
		return {
			title: t("Paused after 3 resumes"),
			body: t(
				"It was resumed 3 times in a row without making progress. Look over where it got to, then resume it."
			),
		};
	}
	return {
		title: t("Stopped"),
		body: t(
			"The conversation stays. Resume starts it again, and so does sending it a message."
		),
	};
};

/**
 * The banner's words for an agent with its account, `where` naming the
 * account's device for the sign-in card.
 */
export const resumeWording = (
	agent: Agent,
	account: Account | undefined,
	where: string
): ResumeWording => {
	if (account !== undefined && needsSignIn(agent, account)) {
		return {
			signIn: true,
			title: t("{{label}} needs a sign-in", { label: account.label }),
			body: t(
				"Sign in from a terminal on {{where}} the way {{label}} asks, then try again. MaxHQ keeps no login for it.",
				{ where: where, label: account.label }
			),
			methods: account.acp?.auth ?? [],
		};
	}
	const { title, body } = stopWording(agent);
	return {
		signIn: false,
		title,
		body: keepsHistory(account) || !account ? body : `${body} ${historyWarning(account.label)}`,
		methods: [],
	};
};

/** Whether the banner shows: a stopped agent MaxHQ holds, or one that needs a sign-in. */
export const showsResume = (agent: Agent, account: Account | undefined): boolean =>
	canResume(agent) || needsSignIn(agent, account);
