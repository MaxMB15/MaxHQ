// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Moving an agent that waits for a usage limit to another account. A new
// agent starts in the same workspace on an account with room left, and its
// first message is a short brief of the old one: what it was asked last,
// what it said last, and the messages that were waiting for it.

import { runningMode } from "@maxhq/shared/agent-view/controls";
import type {
	Account,
	Agent,
	AgentSpec,
	DeviceId,
	TimelineItem,
	UserInput,
} from "@maxhq/shared/bindings";
import { vendorHasMode } from "@maxhq/shared/settings/permission";
import { accountKind, kindName, ownKind } from "@maxhq/shared/status/agent-kind";
import { accountName, atLimit, shortWindow } from "@maxhq/shared/status/usage";
import { t as translate } from "@maxhq/shared/i18n";

/** How many of your last messages the brief repeats. */
const ASKED = 3;

const ASK_LENGTH = 600;
const SAID_LENGTH = 1500;

// The room left in an account's short window, or -1 when it didn't say.
const room = (account: Account): number => {
	const percent = shortWindow(account)?.percent;
	return percent === null || percent === undefined ? -1 : 100 - percent;
};

/**
 * The account an agent can move to: another one on the same device that
 * isn't at its limit, with the most room left in its short window.
 */
export const moveTarget = (
	accounts: Iterable<Account>,
	agent: Pick<Agent, "account">,
	device: DeviceId
): Account | undefined => {
	let best: Account | undefined;
	for (const account of accounts) {
		if (account.device !== device || account.id === agent.account || atLimit(account)) continue;
		if (!best || room(account) > room(best)) best = account;
	}
	return best;
};

/** What the move note says about the account's room, such as "38% of its 5 hours". */
export const roomLeft = (account: Account): string | null => {
	const window = shortWindow(account);
	if (!window || window.percent === null) return null;
	return translate("{{value1}}% of its {{value2}}", {
		value1: Math.max(0, 100 - Math.round(window.percent)),
		value2: window.label.toLowerCase(),
	});
};

const clip = (text: string, max: number): string => {
	const trimmed = text.trim();
	return trimmed.length <= max ? trimmed : `${trimmed.slice(0, max).trimEnd()}…`;
};

const oneLine = (text: string, max: number): string => clip(text.replace(/\s+/g, " "), max);

/**
 * The first message for the new agent. `items` is the old agent's timeline
 * in order, and `waiting` the messages it hadn't sent yet.
 */
export const briefOf = (
	agent: Pick<Agent, "title" | "vendor">,
	account: Account | undefined,
	items: readonly TimelineItem[],
	waiting: readonly TimelineItem[]
): string => {
	const asked: string[] = [];
	let said: string | null = null;
	for (const item of items) {
		if (item.parent !== null) continue;
		if (item.kind === "user_message" && item.delivery !== "queued" && item.text.trim())
			asked.push(item.text);
		if (item.kind === "assistant_message" && item.text.trim()) said = item.text;
	}

	const limit = account ? `${accountName(account)} hit its usage limit` : "it hit a usage limit";
	const parts = [
		`You're taking over from a ${kindName(account ? accountKind(account) : ownKind(agent))} agent called "${agent.title}" in this workspace. It stopped because ${limit}.`,
	];
	if (asked.length > 0) {
		parts.push(
			`The last things it was asked:\n${asked
				.slice(-ASKED)
				.map((text) => `- ${oneLine(text, ASK_LENGTH)}`)
				.join("\n")}`
		);
	}
	if (said !== null) parts.push(`The last thing it said:\n${clip(said, SAID_LENGTH)}`);
	const queued = waiting.flatMap((item) =>
		item.kind === "user_message" && item.text.trim() ? [item.text] : []
	);
	if (queued.length > 0) {
		parts.push(
			`Messages that were waiting for it:\n${queued
				.map((text) => `- ${oneLine(text, ASK_LENGTH)}`)
				.join("\n")}`
		);
	}
	parts.push("Check the workspace for its changes, then carry on from there.");
	return parts.join("\n\n");
};

/** The spec that starts the new agent where the old one runs. */
export const moveSpec = (agent: Agent, target: Account, first: string): AgentSpec => {
	const mode = agent.permission ? runningMode(agent.permission) : null;
	const input: UserInput = { text: first, attachments: [] };
	return {
		vendor: target.vendor,
		account: target.id,
		place: { type: "workspace", workspace: agent.workspace },
		model: null,
		// Codex has no plan mode, nor do some local ACP agents, so a mode the
		// target can't run falls back to the device's default.
		permission: mode !== null && vendorHasMode(target.vendor, mode, target.acp) ? mode : null,
		title: agent.title,
		first: input,
		branch: null,
	};
};
