// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Local ACP agents: commands you set up on one device, each of which shows
// as one account with vendor `acp`. This holds the form for one, what its
// last start found, what it can't do that others can, and the sign-in card
// an agent of it shows. A `maxd` from before them leaves the flag out of
// its hello, so the app shows none of it there.

import { t } from "../i18n";

import type { Account, AcpAgent, AcpAgentId, AcpEnvValue, Agent, DeviceId } from "../bindings";
import { shellQuote } from "../agent-tools/scope";
import { plural, sentence } from "../status/format";
import { all, type ViewState } from "../view/replica";
import { deviceName, linkForDevice } from "../view/select";

/** Whether a device's `maxd` runs local ACP agents, as its hello said. */
export const runsAcpAgents = (view: ViewState, device: DeviceId): boolean =>
	linkForDevice(view, device)?.capabilities?.acp_agents === true;

/** The ACP agents set up on a device, by name. */
export const acpAgentsOn = (view: ViewState, device: DeviceId): AcpAgent[] =>
	all(view, "acp_agent")
		.filter((a) => a.device === device)
		.sort((a, b) => a.name.localeCompare(b.name) || a.id.localeCompare(b.id));

/**
 * The account an agent's sessions run on: the device's `acp` account with
 * its name. The form keeps names unique on a device, so the name finds it.
 */
export const accountOfAgent = (view: ViewState, agent: AcpAgent): Account | undefined =>
	all(view, "account").find(
		(a) => a.vendor === "acp" && a.device === agent.device && a.label === agent.name
	);

/** The definition an `acp` account stands for. */
export const agentOfAccount = (
	view: ViewState,
	account: Pick<Account, "vendor" | "device" | "label">
): AcpAgent | undefined =>
	account.vendor === "acp"
		? acpAgentsOn(view, account.device).find((a) => a.name === account.label)
		: undefined;

/** One argument in the form. */
export type ArgumentRow = { readonly key: string; readonly value: string };

/**
 * One variable in the form. Its value never sits here: the row's field
 * holds what you type until Save reads it, sends it and clears it.
 */
export type VariableRow = {
	/** Stable for the row's life, so its field stays put. */
	readonly key: string;
	readonly name: string;
	/** Whether the device keeps a value for it already. */
	readonly held: boolean;
	/** Whether Save sends a new value. A row the device doesn't hold always does. */
	readonly replacing: boolean;
};

/** The form for one ACP agent. */
export type AcpDraft = {
	/** The agent the form changes, or null for a new one. */
	readonly id: AcpAgentId | null;
	readonly name: string;
	readonly program: string;
	readonly args: readonly ArgumentRow[];
	readonly env: readonly VariableRow[];
	readonly containers: boolean;
};

let rows = 0;

/** A key no other row has. */
export const rowKey = (): string => `row-${++rows}`;

/** An empty argument row. */
export const newArgument = (): ArgumentRow => ({ key: rowKey(), value: "" });

/** An empty variable row, whose value Save sends. */
export const newVariable = (): VariableRow => ({
	key: rowKey(),
	name: "",
	held: false,
	replacing: true,
});

/** The form for `agent` as the device has it, or an empty one for a new agent. */
export const draftOf = (agent?: AcpAgent): AcpDraft => ({
	id: agent?.id ?? null,
	name: agent?.name ?? "",
	program: agent?.program ?? "",
	args: (agent?.args ?? []).map((value) => ({ key: rowKey(), value })),
	env: (agent?.env ?? []).map((name) => ({
		key: rowKey(),
		name,
		held: true,
		replacing: false,
	})),
	containers: agent?.containers ?? false,
});

/** Whether a row's value is typed in a field, so Save reads it. */
export const sendsValue = (row: VariableRow): boolean => !row.held || row.replacing;

const VARIABLE = /^[A-Za-z_][A-Za-z0-9_]*$/;

/** What Save sends, or why it can't. */
export type AcpSave =
	| { readonly ok: true; readonly agent: AcpAgent; readonly env: AcpEnvValue[] }
	| { readonly ok: false; readonly reason: string };

/**
 * The agent and values a save sends, checked the way the device checks
 * them, plus a name no other agent on the device has. `typed` holds each
 * row's field, by row key, read just now. Values go only for new rows and
 * ones being replaced; a row left out is removed with its value.
 */
export const planAcpSave = (
	view: ViewState,
	device: DeviceId,
	draft: AcpDraft,
	id: AcpAgentId,
	typed: ReadonlyMap<string, string>
): AcpSave => {
	const name = draft.name.trim();
	const program = draft.program.trim();
	if (!name) return { ok: false, reason: t("Give the agent a name.") };
	if (!program) return { ok: false, reason: t("Give the program to run.") };
	const taken = acpAgentsOn(view, device).some((a) => a.id !== id && a.name === name);
	if (taken) {
		return {
			ok: false,
			reason: t("{{deviceName_view__device}} already has an agent called {{name}}.", {
				deviceName_view__device: deviceName(view, device),
				name: name,
			}),
		};
	}
	const names: string[] = [];
	const env: AcpEnvValue[] = [];
	for (const row of draft.env) {
		const variable = row.name.trim();
		const value = sendsValue(row) ? (typed.get(row.key) ?? "").trim() : "";
		// A row you added and left blank is nothing to save.
		if (!variable && !row.held && !value) continue;
		if (!VARIABLE.test(variable)) {
			return {
				ok: false,
				reason: variable
					? t(
							"{{variable}} isn't a variable name. Use letters, digits and _, and don't start with a digit.",
							{ variable: variable }
						)
					: t("Give each variable a name."),
			};
		}
		if (names.includes(variable))
			return {
				ok: false,
				reason: t("{{variable}} is listed twice.", { variable: variable }),
			};
		names.push(variable);
		if (!sendsValue(row)) continue;
		if (!value)
			return {
				ok: false,
				reason: t("Type a value for {{variable}}.", { variable: variable }),
			};
		env.push({ name: variable, value });
	}
	return {
		ok: true,
		agent: {
			id,
			device,
			name,
			program,
			args: draft.args.map((a) => a.value).filter((value) => value !== ""),
			env: names,
			containers: draft.containers,
		},
		env,
	};
};

/** What the agent's last start found, in a line, or why it failed. */
export const probeLine = (
	agent: AcpAgent,
	account: Account | undefined
): { readonly text: string; readonly error: boolean } => {
	if (agent.problem) return { text: `${sentence(agent.problem)}.`, error: true };
	const features = account?.acp;
	if (!features) return { text: t("Not checked yet. Saving it checks it."), error: false };
	const models = account.models.length;
	const parts = [
		features.images ? t("Takes images") : t("Takes images as file paths"),
		features.load ? t("keeps history on restart") : t("starts fresh on restart"),
		...(features.plan ? [t("has a plan mode")] : []),
		models > 0 ? plural(models, "model") : t("runs its default model"),
	];
	return { text: `${parts.join(", ")}.`, error: false };
};

/** Whether a restart keeps the agent's history: not for a local ACP agent that can't load a session. */
export const keepsHistory = (account: Account | undefined): boolean =>
	account?.vendor !== "acp" || account.acp?.load === true;

/** What a restart of an agent that can't keep its history warns. */
export const historyWarning = (name: string): string =>
	t(
		"{{name}} can't load an earlier session, so it starts again without this conversation's history.",
		{ name: name }
	);

/** Whether the agent's CLI takes images inline: not a local ACP agent that didn't say so. */
export const takesImages = (account: Account | undefined): boolean =>
	account?.vendor !== "acp" || account.acp?.images === true;

/** What the composer says under an image an agent takes only as a path. */
export const imagesNote = (name: string): string =>
	t("{{name}} doesn't take images, so they go as file paths in the message.", { name: name });

/** What `maxd` says when a local ACP agent needs a sign-in. */
export const signInReason = (name: string): string =>
	t("{{name}} needs a sign-in.", { name: name });

/** Whether an agent stopped because its local ACP agent needs a sign-in. */
export const needsSignIn = (agent: Agent, account: Account | undefined): boolean =>
	agent.vendor === "acp" &&
	agent.activity.state === "error" &&
	account !== undefined &&
	agent.activity.reason === signInReason(account.label);

/** The agent's program and arguments as one line for a shell, to sign in from. */
export const signInCommand = (agent: AcpAgent): string =>
	[agent.program, ...(agent.args ?? [])].map(shellQuote).join(" ");
