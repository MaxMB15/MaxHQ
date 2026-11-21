// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the views show for local models: a device's servers, catalog and
// model files with their fit, the Local entry in the agent picker with its
// models grouped by server, and a local agent's name, "Local · <model>".
// The picker sends a local model as one value that names both the account
// that reaches the server and the model, so one Local entry can offer every
// server's models.

import { t } from "../i18n";

import {
	catalogEntry,
	catalogIdOf,
	defaultLocalPick,
	localAccountsOn,
	localModelsOf,
	localSettingsOf,
	serverModelName,
	serverOf,
	usableServer,
	type LocalPick,
	type LocalSettings,
} from "./models";
import { estimate, recommendedFile, type Estimate } from "./fit";
import {
	localAgentLabel,
	loadText,
	notEncrypted,
	notEncryptedText,
	serverProblemText,
} from "./wording";
import { agentKind, kindName, type AccountLookup } from "../status/agent-kind";
import { all, type ViewState } from "../view/replica";
import { linkForDevice } from "../view/select";
import type {
	Account,
	AccountId,
	Agent,
	CatalogFile,
	CatalogModel,
	DeviceId,
	ModelFile,
	ModelServer,
} from "../bindings";
import type { DeviceFields } from "../settings/device-settings";

export {
	catalogIdOf,
	DEFAULT_CATALOG_MODEL,
	defaultLocalPick,
	localAccountsOn,
	localModelsOf,
	localPicksOn,
	localSettingsOf,
	LOCAL_MODEL_DEFAULTS,
	managedModelId,
	serverOf,
	usableServer,
	type LocalPick,
	type LocalSettings,
} from "./models";

/** Whether a device's `maxd` runs local models: their servers, downloads and loads. */
export const runsLocalModels = (view: ViewState, device: DeviceId): boolean =>
	linkForDevice(view, device)?.capabilities?.local_models === true;

/** A device's local model settings, with the defaults filled in. */
export const localSettingsOn = (view: ViewState, device: DeviceId): LocalSettings =>
	localSettingsOf(view.tables.device_settings.get(device)?.local_models);

/**
 * The servers a device's `maxd` runs or reaches by URL, as its Settings page
 * lists them: the managed one first, then URLs by label.
 */
export const serversOf = (view: ViewState, device: DeviceId): ModelServer[] =>
	all(view, "model_server")
		.filter((s) => s.device === device)
		.sort(
			(a, b) =>
				Number(a.kind.type !== "managed") - Number(b.kind.type !== "managed") ||
				a.label.localeCompare(b.label) ||
				a.id.localeCompare(b.id)
		);

/** The devices whose local agents use a server, other than its own: those that reach it over the link. */
export const serverUsers = (
	view: ViewState,
	server: Pick<ModelServer, "id" | "device">
): DeviceId[] =>
	[
		...new Set(
			all(view, "account")
				.filter((a) => a.model_server === server.id && a.device !== server.device)
				.map((a) => a.device)
		),
	].sort();

/** The model files a device holds or is downloading. */
export const modelFilesOn = (view: ViewState, device: DeviceId): readonly ModelFile[] =>
	localModelsOf(view, device)?.files ?? [];

/** The files a device is downloading or has paused, for a download indicator. */
export const downloadsOn = (view: ViewState, device: DeviceId): ModelFile[] =>
	modelFilesOn(view, device).filter(
		(f) => f.state.state === "downloading" || f.state.state === "paused"
	);

/** One catalog model as a device's Settings page lists it. */
export type CatalogRow = {
	/** The catalog model. */
	readonly model: CatalogModel;
	/** The file to show: the one on disk or on its way, else the recommended one. */
	readonly file: CatalogFile;
	/** That file on the device, once it's downloading or downloaded. */
	readonly onDisk: ModelFile | undefined;
	/** Its fit and speed on the device, or null without a GPU. */
	readonly estimate: Estimate | null;
	/** Whether it's the catalog's default pick. */
	readonly isDefault: boolean;
};

/**
 * A device's catalog with each model's fit and speed for its settings,
 * best pick first. Empty for a device that can't run a managed server, which
 * recommends no models.
 */
export const catalogRows = (view: ViewState, device: DeviceId): CatalogRow[] => {
	const local = localModelsOf(view, device);
	if (!local || local.runtime.support !== "managed") return [];
	const settings = localSettingsOn(view, device);
	return local.catalog.models.flatMap((model, index) => {
		const onDisk = local.files.find((f) => f.model === model.id);
		const file =
			(onDisk && model.files.find((f) => f.name === onDisk.file)) ?? recommendedFile(model);
		if (!file) return [];
		return [
			{
				model,
				file,
				onDisk,
				estimate: estimate(local.hardware, model, file, {
					context: settings.context,
					kvType: settings.kv_type,
				}),
				isDefault: index === 0,
			},
		];
	});
};

/** The prefix of a picker value that names a local account and a model. */
const KEY_PREFIX = "local|";

/** The picker value for a model on a local account. */
export const localModelKey = (account: AccountId, model: string): string =>
	`${KEY_PREFIX}${account}|${model}`;

/** The account and model a picker value names, when it names a local model. */
export const parseLocalModelKey = (
	value: string | null | undefined
): { readonly account: AccountId; readonly model: string } | null => {
	if (!value?.startsWith(KEY_PREFIX)) return null;
	const rest = value.slice(KEY_PREFIX.length);
	const bar = rest.indexOf("|");
	if (bar <= 0 || bar === rest.length - 1) return null;
	return { account: rest.slice(0, bar), model: rest.slice(bar + 1) };
};

/**
 * What picking a model in the picker changes: the model, and for a local
 * model the account that reaches its server too.
 */
export const modelChoice = (
	value: string
): { readonly account?: AccountId; readonly model: string | null } => {
	const local = parseLocalModelKey(value);
	return local ? { account: local.account, model: value } : { model: value || null };
};

/**
 * The local account and model a picked value lands on: the picked model
 * while its account is a local account on the device and it lists it, else
 * `account`'s own model when it lists it, else the device's default pick
 * when it's on `account`, else `account`'s first model.
 */
export const resolveLocal = (
	view: ViewState,
	device: DeviceId,
	account: Account,
	wanted: string | null
): { readonly account: Account; readonly model: string | null } => {
	const listed = (a: Account, model: string): boolean =>
		serverOf(view, a)?.models.some((m) => m.id === model) === true ||
		a.models.some((m) => m.id === model);
	const picked = parseLocalModelKey(wanted);
	if (picked) {
		const other = localAccountsOn(view, device).find((a) => a.id === picked.account);
		if (other && listed(other, picked.model)) return { account: other, model: picked.model };
	}
	if (wanted && !picked && listed(account, wanted)) return { account, model: wanted };
	const pick = defaultLocalPick(view, device);
	if (pick?.account.id === account.id) return { account, model: pick.model.id };
	const first = serverOf(view, account)?.models[0]?.id ?? account.models[0]?.id ?? null;
	return { account, model: first };
};

/** The value the picker shows for a model on a local account. */
export const shownLocalModel = (account: Account, model: string | null): string | null =>
	model === null ? null : localModelKey(account.id, model);

/** A picker entry, as `new-agent/spec` defines it. */
type PickerOption = {
	readonly value: string;
	readonly label: string;
	readonly detail?: string;
	readonly warn?: boolean;
	readonly tag?: string;
	readonly disabled?: boolean;
	readonly group?: string;
};

/** The value of the Local entry on a device that has no local model yet. */
export const LOCAL_SETUP_VALUE = "local-setup";

/**
 * The Local entry in the agent picker, or null when the device has no
 * local models. It stands for every local account on the device; its value
 * is `selected` when that's one of them, else the default pick's account.
 * A Mac that could run models but has none ready gets a turned-off entry
 * that says to set one up.
 */
export const localAccountOption = (
	view: ViewState,
	device: DeviceId,
	selected: AccountId | null = null
): PickerOption | null => {
	const accounts = localAccountsOn(view, device);
	if (accounts.length === 0) {
		const local = localModelsOf(view, device);
		if (!local || local.runtime.support !== "managed" || !runsLocalModels(view, device))
			return null;
		return {
			value: LOCAL_SETUP_VALUE,
			label: kindName("local"),
			detail: t("Download a model in Settings first"),
			disabled: true,
		};
	}
	const pick = defaultLocalPick(view, device);
	const value =
		accounts.find((a) => a.id === selected)?.id ?? pick?.account.id ?? accounts[0]!.id;
	return {
		value,
		label: kindName("local"),
		detail: pick
			? t("{{model}} on {{server}}", {
					model: serverModelName(view, pick.server, pick.model),
					server: pick.server.label,
				})
			: t("No model ready yet"),
		warn: pick === null,
	};
};

/**
 * The models every local account on a device offers, grouped by server,
 * each value naming its account and model. A server that can't run a Local
 * agent shows its models turned off, with why.
 */
export const localModelOptions = (view: ViewState, device: DeviceId): PickerOption[] => {
	const pick = defaultLocalPick(view, device);
	return localAccountsOn(view, device).flatMap((account): PickerOption[] => {
		const server = serverOf(view, account);
		if (!server) return [];
		const problem = serverProblemText(server);
		const usable = usableServer(server);
		const group = notEncrypted(server)
			? t("{{server}} (not encrypted)", { server: server.label })
			: server.label;
		if (server.models.length === 0)
			return [
				{
					value: localModelKey(account.id, ""),
					label: t("No models"),
					detail: problem ?? t("This server lists no models"),
					disabled: true,
					group,
				},
			];
		return server.models.map((model) => {
			const isDefault = pick?.account.id === account.id && pick.model.id === model.id;
			const load = loadText(model.load);
			const notes = [isDefault ? t("The default") : null, load].filter(
				(note): note is string => note !== null
			);
			return {
				value: localModelKey(account.id, model.id),
				label: serverModelName(view, server, model),
				detail: usable ? notes.join(", ") || undefined : (problem ?? undefined),
				warn: !usable,
				disabled: !usable,
				group,
			};
		});
	});
};

/** The note for a server row whose traffic crosses the network unencrypted, or null. */
export const serverNote = (server: ModelServer): string | null =>
	notEncrypted(server) ? notEncryptedText() : null;

/** The model a local agent runs, as the catalog or its server names it. */
export const localModelName = (
	view: ViewState,
	agent: Pick<Agent, "account" | "model">
): string | null => {
	if (agent.model === null) return null;
	const account = agent.account ? view.tables.account.get(agent.account) : undefined;
	const server = account ? serverOf(view, account) : undefined;
	const model = server?.models.find((m) => m.id === agent.model);
	if (server && model) return serverModelName(view, server, model);
	const id = catalogIdOf(agent.model);
	for (const local of view.tables.local_models.values()) {
		const entry = local.catalog.models.find((m) => m.id === id);
		if (entry) return entry.name;
	}
	return agent.model;
};

/**
 * What an agent goes by wherever it shows: "Local · <model>" for a local
 * agent, else its vendor's name, such as "Codex".
 */
export const agentKindLabel = (
	view: ViewState,
	agent: Pick<Agent, "vendor" | "account" | "local" | "model">
): string => {
	const kind = agentKind(agent, view.tables.account as AccountLookup);
	return kind === "local" ? localAgentLabel(localModelName(view, agent)) : kindName(kind);
};

/** The catalog entry a local agent's model runs, when it's a catalog model. */
export const agentCatalogModel = (
	view: ViewState,
	agent: Pick<Agent, "account" | "model">
): CatalogModel | undefined => {
	const account = agent.account ? view.tables.account.get(agent.account) : undefined;
	const server = account ? serverOf(view, account) : undefined;
	const model = server?.models.find((m) => m.id === agent.model);
	return server && model ? catalogEntry(view, server, model) : undefined;
};

/** Device settings with a new default local model, ready to save. */
export const withDefaultLocalModel = (
	settings: DeviceFields,
	pick: Pick<LocalPick, "server" | "model"> | null
): DeviceFields => ({
	...settings,
	local_models: {
		...settings.local_models,
		default_model: pick ? { server: pick.server.id, model: pick.model.id } : null,
	},
});
