// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What kind of agent a person sees. On the wire a local agent is a Claude
// agent whose account logs in to a model server, but everywhere it shows it
// is Local: its own name, its own icon, never Claude's, and never in usage
// meters or continue-at-limit. Every vendor name and icon goes through
// `agentKind` or `accountKind` so no view can miss it.

import { t } from "../i18n";

import type { Account, AccountId, Agent, Vendor } from "../bindings";

/** What a person sees an agent as: its vendor, or Local. */
export type AgentKind = Vendor | "local";

/** Every kind, in the order pickers and lists show them. */
export const AGENT_KINDS: readonly AgentKind[] = ["claude", "codex", "cursor", "local", "acp"];

/** What the vendor's CLI is called, in the header meter and elsewhere. */
export const VENDOR_NAME: Record<Vendor, string> = {
	claude: "Claude",
	codex: "Codex",
	cursor: "Cursor",
	get acp() {
		return t("ACP agent");
	},
};

/** The name a kind goes by, such as "Codex" or "Local". */
export const kindName = (kind: AgentKind): string =>
	kind === "local" ? t("Local") : VENDOR_NAME[kind];

/** Whether an account runs a local model, through a model server. */
export const isLocalAccount = (account: Pick<Account, "login"> | null | undefined): boolean =>
	account?.login === "model_server";

/** What an account's agents show as. */
export const accountKind = (account: Pick<Account, "vendor" | "login">): AgentKind =>
	isLocalAccount(account) ? "local" : account.vendor;

/** The accounts to look an agent's account up in: a view's table or a list. */
export type AccountLookup =
	ReadonlyMap<string, Pick<Account, "login">> | readonly Pick<Account, "id" | "login">[];

const findAccount = (accounts: AccountLookup, id: AccountId): Pick<Account, "login"> | undefined =>
	"get" in accounts ? accounts.get(id) : accounts.find((a) => a.id === id);

/**
 * What an agent shows as from its own record alone: Local once its model
 * server reports on it, else its vendor. For an agent whose account no view
 * reaches, such as one running in a terminal; `agentKind` is the full check.
 */
export const ownKind = (agent: Pick<Agent, "vendor" | "local">): AgentKind =>
	agent.local != null ? "local" : agent.vendor;

/**
 * What an agent shows as. A local agent is one whose model server reports
 * on it, or whose account logs in to a model server; it's never Claude.
 */
export const agentKind = (
	agent: Pick<Agent, "vendor" | "account" | "local">,
	accounts: AccountLookup
): AgentKind => {
	if (agent.account !== null && isLocalAccount(findAccount(accounts, agent.account)))
		return "local";
	return ownKind(agent);
};

/** Whether an agent shows as Local. */
export const isLocalAgent = (
	agent: Pick<Agent, "vendor" | "account" | "local">,
	accounts: AccountLookup
): boolean => agentKind(agent, accounts) === "local";

/** The name an agent's kind goes by, such as "Claude" or "Local". */
export const agentKindName = (
	agent: Pick<Agent, "vendor" | "account" | "local">,
	accounts: AccountLookup
): string => kindName(agentKind(agent, accounts));
