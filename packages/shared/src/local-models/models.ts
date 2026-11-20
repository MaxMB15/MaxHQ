// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The servers and models a device's local agents run on. A local agent's
// account names a model server: its own device's managed server, a server
// URL you gave, or another device's managed server reached over the maxd
// link. The models to offer come from that server, named as the catalog
// names them, and a device's default pick comes from its settings, else
// the catalog's default model, else the first model ready.

import { isLocalAccount } from "../status/agent-kind";
import { all, type ViewState } from "../view/replica";
import type {
	Account,
	CatalogModel,
	DeviceId,
	LocalModelSettings,
	LocalModels,
	ModelChoice,
	ModelServer,
	ServerModel,
} from "../bindings";

/** The catalog model a device picks when nothing else says. */
export const DEFAULT_CATALOG_MODEL = "qwen3.6-35b-a3b";

/** A device's local model settings with every field filled in. */
export type LocalSettings = Required<LocalModelSettings>;

/** The settings a device has before you change any, the same as `maxd`'s. */
export const LOCAL_MODEL_DEFAULTS: LocalSettings = {
	idle_unload_minutes: 15,
	kv_type: "f16",
	context: 65_536,
	fewer_tools: true,
	default_model: null,
};

/** Local model settings with the defaults filled in for any left out. */
export const localSettingsOf = (kept: LocalModelSettings | undefined): LocalSettings => ({
	idle_unload_minutes: kept?.idle_unload_minutes ?? LOCAL_MODEL_DEFAULTS.idle_unload_minutes,
	kv_type: kept?.kv_type ?? LOCAL_MODEL_DEFAULTS.kv_type,
	context: kept?.context ?? LOCAL_MODEL_DEFAULTS.context,
	fewer_tools: kept?.fewer_tools ?? LOCAL_MODEL_DEFAULTS.fewer_tools,
	default_model: kept?.default_model ?? null,
});

/** Whether two sets of local model settings say the same. */
export const sameLocalSettings = (a: LocalModelSettings, b: LocalModelSettings): boolean => {
	const x = localSettingsOf(a);
	const y = localSettingsOf(b);
	return (
		x.idle_unload_minutes === y.idle_unload_minutes &&
		x.kv_type === y.kv_type &&
		x.context === y.context &&
		x.fewer_tools === y.fewer_tools &&
		x.default_model?.server === y.default_model?.server &&
		x.default_model?.model === y.default_model?.model
	);
};

/** What a device knows for local models, when its `maxd` reports it. */
export const localModelsOf = (view: ViewState, device: DeviceId): LocalModels | undefined =>
	view.tables.local_models.get(device);

/** The server a local account runs on. */
export const serverOf = (
	view: ViewState,
	account: Pick<Account, "model_server">
): ModelServer | undefined =>
	account.model_server ? view.tables.model_server.get(account.model_server) : undefined;

/** The catalog entry a server's model runs, from the catalog of the server's device. */
export const catalogEntry = (
	view: ViewState,
	server: Pick<ModelServer, "device">,
	model: Pick<ServerModel, "catalog">
): CatalogModel | undefined => {
	if (!model.catalog) return undefined;
	const id = model.catalog.model;
	return localModelsOf(view, server.device)?.catalog.models.find((m) => m.id === id);
};

/** A server's model as it's named: the catalog's name, else the server's. */
export const serverModelName = (
	view: ViewState,
	server: Pick<ModelServer, "device">,
	model: ServerModel
): string => catalogEntry(view, server, model)?.name ?? model.label;

/**
 * Whether a server can run a Local agent: not before a probe says it can't,
 * and only with the Anthropic Messages API.
 */
export const usableServer = (server: Pick<ModelServer, "probe">): boolean =>
	server.probe === null || (server.probe.error === null && server.probe.apis.messages === true);

/**
 * The local accounts on a device: the ones on its own servers first, a
 * managed one before URLs, then the ones that reach another device's
 * server, each by label.
 */
export const localAccountsOn = (view: ViewState, device: DeviceId): Account[] => {
	const rank = (account: Account): number => {
		const server = serverOf(view, account);
		if (!server) return 3;
		if (server.device !== device) return 2;
		return server.kind.type === "managed" ? 0 : 1;
	};
	return all(view, "account")
		.filter((a) => a.device === device && isLocalAccount(a))
		.sort(
			(a, b) =>
				rank(a) - rank(b) || a.label.localeCompare(b.label) || a.id.localeCompare(b.id)
		);
};

/** One model a local agent on a device can start on. */
export type LocalPick = {
	/** The local account that reaches the server. */
	readonly account: Account;
	/** The server. */
	readonly server: ModelServer;
	/** The model on it. */
	readonly model: ServerModel;
};

/** Every model a device's local agents can start on, by account and then as the server lists them. */
export const localPicksOn = (view: ViewState, device: DeviceId): LocalPick[] =>
	localAccountsOn(view, device).flatMap((account) => {
		const server = serverOf(view, account);
		if (!server || !usableServer(server)) return [];
		return server.models.map((model) => ({ account, server, model }));
	});

/**
 * The model a new local agent on a device starts on: the device's default
 * while it's there, else the catalog's default model, else the first one.
 */
export const defaultLocalPick = (view: ViewState, device: DeviceId): LocalPick | null => {
	const picks = localPicksOn(view, device);
	const wanted = view.tables.device_settings.get(device)?.local_models?.default_model;
	return (
		(wanted &&
			picks.find((p) => p.server.id === wanted.server && p.model.id === wanted.model)) ||
		picks.find((p) => p.model.catalog?.model === DEFAULT_CATALOG_MODEL) ||
		picks[0] ||
		null
	);
};

/**
 * The models to offer for a local account: what its `maxd` listed, else
 * its server's, with the device's default pick marked. Local models take no
 * effort levels, and `maxd` sends them an attached image by its path.
 */
export const serverModelChoices = (view: ViewState, account: Account): readonly ModelChoice[] => {
	if (account.models.length > 0) return account.models;
	const server = serverOf(view, account);
	if (!server) return [];
	const pick = defaultLocalPick(view, account.device);
	const mine = pick?.account.id === account.id;
	return server.models.map((model, index) => ({
		id: model.id,
		label: serverModelName(view, server, model),
		default: mine ? pick.model.id === model.id : index === 0,
		efforts: [],
		default_effort: null,
		no_images: true,
	}));
};
