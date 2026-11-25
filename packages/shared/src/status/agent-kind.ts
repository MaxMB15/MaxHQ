// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What kind of agent a person sees. On the wire a local agent is a Claude
// agent whose account logs in to a model server, but everywhere it shows it
// is Local: its own name, its own icon, never Claude's, and never in usage
// meters or continue-at-limit. Every vendor name and icon goes through
// `agentKind` or `accountKind` so no view can miss it. A local agent on a
// cloud service, such as OpenRouter, is still Local to the logic, but it
// goes by the service's name and shows a cloud, as the `servers` the name
// and mark helpers take say.

import { t } from "../i18n";

import type { Account, AccountId, Agent, ModelServer, Vendor } from "../bindings";

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

/** An account as the kind helpers read it: its login, and its server when it has one. */
export type KindAccount = Pick<Account, "login"> & Partial<Pick<Account, "model_server">>;

/** A model server as the names read it: what it's called, and whether it's a cloud service. */
export type ServerName = Pick<ModelServer, "label"> & Partial<Pick<ModelServer, "cloud">>;

/** The servers to look a local account's server up in: a view's table. */
export type ServerLookup = ReadonlyMap<string, ServerName>;

/**
 * The cloud service a local account runs on, or undefined for one on
 * hardware you run. A server from a `maxd` older than cloud servers says
 * nothing and stays Local.
 */
export const cloudServerOf = (
	account: Partial<KindAccount> | null | undefined,
	servers: ServerLookup | undefined
): ServerName | undefined => {
	if (account?.login !== "model_server" || !account.model_server) return undefined;
	const server = servers?.get(account.model_server);
	return server?.cloud === true ? server : undefined;
};

/** The mark an agent or account shows: its kind's, or a cloud for a cloud service's. */
export type KindMark = AgentKind | "cloud";

/** What an account's agents show as. */
export const accountKind = (account: Pick<Account, "vendor" | "login">): AgentKind =>
	isLocalAccount(account) ? "local" : account.vendor;

/** The mark an account's agents show: a cloud for a cloud service's, else their kind's. */
export const accountMark = (
	account: Pick<Account, "vendor"> & KindAccount,
	servers: ServerLookup | undefined
): KindMark => (cloudServerOf(account, servers) ? "cloud" : accountKind(account));

/**
 * The name an account's agents go by: a cloud service's own, such as
 * "OpenRouter", else their kind's, such as "Codex" or "Local".
 */
export const accountKindName = (
	account: Pick<Account, "vendor"> & KindAccount,
	servers: ServerLookup | undefined
): string => cloudServerOf(account, servers)?.label ?? kindName(accountKind(account));

/** The accounts to look an agent's account up in: a view's table or a list. */
export type AccountLookup =
	ReadonlyMap<string, KindAccount> | readonly (Pick<Account, "id"> & KindAccount)[];

const findAccount = (accounts: AccountLookup, id: AccountId): KindAccount | undefined =>
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

/** The cloud service an agent runs on, or undefined for any other agent. */
export const agentCloudServer = (
	agent: Pick<Agent, "account">,
	accounts: AccountLookup,
	servers: ServerLookup | undefined
): ServerName | undefined =>
	agent.account === null
		? undefined
		: cloudServerOf(findAccount(accounts, agent.account), servers);

/**
 * The name an agent's kind goes by, such as "Claude" or "Local", or its
 * cloud service's, such as "OpenRouter", when `servers` says it runs on one.
 */
export const agentKindName = (
	agent: Pick<Agent, "vendor" | "account" | "local">,
	accounts: AccountLookup,
	servers?: ServerLookup
): string =>
	agentCloudServer(agent, accounts, servers)?.label ?? kindName(agentKind(agent, accounts));

/** The mark an agent shows: a cloud for a cloud service's, else its kind's. */
export const agentMark = (
	agent: Pick<Agent, "vendor" | "account" | "local">,
	accounts: AccountLookup,
	servers: ServerLookup | undefined
): KindMark => (agentCloudServer(agent, accounts, servers) ? "cloud" : agentKind(agent, accounts));
