// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The servers and models a device's local agents run on. A local agent's
// account names a model server: its own device's managed server, a server
// URL you gave, or another device's managed server reached over the maxd
// link. The models to offer come from that server, named as the catalog
// names them, and a device's default pick comes from its settings, else
// the catalog's default model, else the first model ready. A cloud service,
// such as OpenRouter, is a server URL too, but it shows as an agent of its
// own rather than under Local, so each of its accounts is an entry apart.

import { isLocalAccount } from "../status/agent-kind";
import { all, type ViewState } from "../view/replica";
import type {
	Account,
	AccountId,
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

/**
 * A catalog file's model ID on a managed server, `<catalog id>@<quant>` in
 * lowercase, such as `qwen3.6-35b-a3b@ud-q4_k_xl`. `maxd` gives it to
 * llama-server as the alias, so the server reports the same ID.
 */
export const managedModelId = (model: string, quant: string): string =>
	`${model}@${quant}`.toLowerCase();

/** The catalog ID in a managed server's model ID, or the ID itself. */
export const catalogIdOf = (id: string): string => id.split("@", 1)[0] ?? id;

/** A device's local model settings with every field filled in. */
export type LocalSettings = Required<LocalModelSettings>;

/** The settings a device has before you change any, the same as `maxd`'s. */
export const LOCAL_MODEL_DEFAULTS: LocalSettings = {
	idle_unload_minutes: 15,
	kv_type: "f16",
	context: 65_536,
	fewer_tools: true,
	linked_devices: true,
	default_model: null,
};

/** Local model settings with the defaults filled in for any left out. */
export const localSettingsOf = (kept: LocalModelSettings | undefined): LocalSettings => ({
	idle_unload_minutes: kept?.idle_unload_minutes ?? LOCAL_MODEL_DEFAULTS.idle_unload_minutes,
	kv_type: kept?.kv_type ?? LOCAL_MODEL_DEFAULTS.kv_type,
	context: kept?.context ?? LOCAL_MODEL_DEFAULTS.context,
	fewer_tools: kept?.fewer_tools ?? LOCAL_MODEL_DEFAULTS.fewer_tools,
	linked_devices: kept?.linked_devices ?? LOCAL_MODEL_DEFAULTS.linked_devices,
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
		x.linked_devices === y.linked_devices &&
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

/**
 * Whether a server is a hosted API rather than hardware you run, as its
 * `maxd` says. One from a `maxd` older than cloud servers says nothing and
 * counts as Local.
 */
export const isCloudServer = (server: Pick<ModelServer, "cloud"> | undefined): boolean =>
	server?.cloud === true;

/** Whether a local account runs on a cloud service. */
export const isCloudAccount = (
	view: ViewState,
	account: Pick<Account, "login" | "model_server">
): boolean => isLocalAccount(account) && isCloudServer(serverOf(view, account));

/** The Agent entry that stands for the local accounts on hardware you run. */
export const LOCAL_ENTRY = "local";

/**
 * The Agent entry a local account shows under: Local, for hardware you run,
 * or a cloud service's account, which is an entry of its own.
 */
export type LocalEntry = typeof LOCAL_ENTRY | AccountId;

/** The Agent entry a local account shows under. */
export const entryOf = (
	view: ViewState,
	account: Pick<Account, "id" | "login" | "model_server">
): LocalEntry => (isCloudAccount(view, account) ? account.id : LOCAL_ENTRY);

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
 * Whether a device's local agents can't use a server because it's another
 * device's managed server and that device turned off "Linked devices can
 * use this Mac's models". Its `maxd` refuses their turns then.
 */
export const sharingOff = (
	view: ViewState,
	device: DeviceId,
	server: Pick<ModelServer, "device" | "kind">
): boolean =>
	server.device !== device &&
	server.kind.type === "managed" &&
	!localSettingsOf(view.tables.device_settings.get(server.device)?.local_models).linked_devices;

/**
 * The local accounts on a device: the ones on its own servers first, a
 * managed one before URLs, then the ones that reach another device's
 * server, each by label. With `entry`, only those under that Agent entry.
 */
export const localAccountsOn = (
	view: ViewState,
	device: DeviceId,
	entry?: LocalEntry
): Account[] => {
	const rank = (account: Account): number => {
		const server = serverOf(view, account);
		if (!server) return 3;
		if (server.device !== device) return 2;
		return server.kind.type === "managed" ? 0 : 1;
	};
	return all(view, "account")
		.filter(
			(a) =>
				a.device === device &&
				isLocalAccount(a) &&
				(entry === undefined || entryOf(view, a) === entry)
		)
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

/**
 * Every model a device's local agents can start on, by account and then as
 * the server lists them, only under `entry` when given. A Mac's server
 * whose switch is off has none.
 */
export const localPicksOn = (view: ViewState, device: DeviceId, entry?: LocalEntry): LocalPick[] =>
	localAccountsOn(view, device, entry).flatMap((account) => {
		const server = serverOf(view, account);
		if (!server || !usableServer(server) || sharingOff(view, device, server)) return [];
		return server.models.map((model) => ({ account, server, model }));
	});

/**
 * The model a new local agent on a device starts on: the device's default
 * while it's there, else the catalog's default model, else the first one.
 * With `entry`, the one under that Agent entry, so a default on a cloud
 * service leaves Local its own.
 */
export const defaultLocalPick = (
	view: ViewState,
	device: DeviceId,
	entry?: LocalEntry
): LocalPick | null => {
	const picks = localPicksOn(view, device, entry);
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
	const pick = defaultLocalPick(view, account.device, entryOf(view, account));
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
