// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Local entries in the desktop's agent and model pickers. Local is an
// agent of its own and always shows, so a device with no model ready yet
// leads to its Local models settings. Its models sit under their server,
// "This Mac" for the Mac the app runs on, each with how well it fits, and a
// cloud server's heading carries its credits; the catalog models not
// downloaded yet follow, and a last row opens the setup.

import { t } from "../i18n";

import { creditsShort } from "./credits";
import { estimate, recommendedFile, type Fit } from "./fit";
import { catalogEntry, localAccountsOn, localModelsOf, serverOf, sharingOff } from "./models";
import {
	catalogRows,
	LOCAL_SETUP_VALUE,
	localAccountOption,
	localModelKey,
	localModelOptions,
	localSettingsOn,
	parseLocalModelKey,
	runsLocalModels,
	serversOf,
} from "./select";
import {
	fileStateText,
	fitLabel,
	formatFileSize,
	linkedDevicesLabel,
	notEncrypted,
} from "./wording";
import { matchesModel } from "../agent-view/model-search";
import type { Account, DeviceId, ModelServer, ServerModel } from "../bindings";
import { accountOptions, type Option } from "../new-agent/spec";
import { isThisMac } from "../settings/app-keys";
import { isLocalAccount, kindName } from "../status/agent-kind";
import type { Tone } from "../tasks/format";
import type { ViewState } from "../view/replica";
import { deviceName } from "../view/select";

/** The color of each fit's tag. */
export const FIT_TONE: Record<Fit, Tone> = { fits: "good", tight: "warn", too_large: "bad" };

/** A tag beside a model in a picker, such as a green "Fits". */
export type ModelTag = { readonly label: string; readonly tone: Tone };

/**
 * How well a server's model fits the GPU of the device that runs it, or
 * null when that isn't known, as for a model a URL serves.
 */
export const serverModelFit = (
	view: ViewState,
	server: Pick<ModelServer, "device">,
	model: ServerModel
): Fit | null => {
	const entry = catalogEntry(view, server, model);
	const local = localModelsOf(view, server.device);
	if (!entry || !local) return null;
	const file = entry.files.find((f) => f.name === model.catalog?.file) ?? recommendedFile(entry);
	if (!file) return null;
	const settings = localSettingsOn(view, server.device);
	return (
		estimate(local.hardware, entry, file, {
			context: settings.context,
			kvType: settings.kv_type,
		})?.fit ?? null
	);
};

/** The fit tag of a server's model, or null without one. */
const fitTag = (
	view: ViewState,
	server: Pick<ModelServer, "device">,
	model: ServerModel
): ModelTag | null => {
	const fit = serverModelFit(view, server, model);
	return fit ? { label: fitLabel(fit), tone: FIT_TONE[fit] } : null;
};

/** The tag of a local account's model in a picker, such as a green "Fits", or null. */
export const localModelTag = (
	view: ViewState,
	account: Account,
	model: string
): ModelTag | null => {
	const server = serverOf(view, account);
	const found = server?.models.find((m) => m.id === model);
	return server && found ? fitTag(view, server, found) : null;
};

/**
 * What a device's models are listed under: "This Mac" for the Mac the app
 * runs on, else the device's name.
 */
const deviceGroup = (view: ViewState, device: DeviceId): string =>
	isThisMac(view, device) ? t("This Mac") : deviceName(view, device);

/**
 * What a server's models are listed under: "This Mac" for the server MaxHQ
 * runs on the Mac the app runs on, else the server's name, noting a server
 * whose traffic isn't encrypted.
 */
export const serverGroup = (view: ViewState, server: ModelServer): string => {
	const name = server.kind.type === "managed" ? deviceGroup(view, server.device) : server.label;
	return notEncrypted(server) ? t("{{server}} (not encrypted)", { server: name }) : name;
};

/**
 * A server's heading in the model pickers: its group, as `serverGroup`
 * says, with the main figure of its credits when it has them, such as
 * "OpenRouter · $7.42 left".
 */
export const serverHeading = (view: ViewState, server: ModelServer): string => {
	const group = serverGroup(view, server);
	const credits = creditsShort(server);
	return credits ? t("{{server}} · {{credits}}", { server: group, credits }) : group;
};

/** The separator in a value that opens the Local models settings. */
const SETUP_BAR = "|";

/**
 * The device whose Local models settings a picked value opens, or null when
 * it picks something else. `fallback` stands for the setup row, which names
 * no device of its own.
 */
export const opensLocalSetup = (
	value: string,
	fallback: DeviceId | null
): { readonly device: DeviceId | null } | null => {
	if (value === LOCAL_SETUP_VALUE) return { device: fallback };
	if (!value.startsWith(LOCAL_SETUP_VALUE + SETUP_BAR)) return null;
	const device = value.slice(LOCAL_SETUP_VALUE.length + 1).split(SETUP_BAR)[0];
	return { device: device || fallback };
};

/**
 * The model a model picker's row names, which its search matches besides
 * the row's name: a local model's ID, the catalog model a row not
 * downloaded yet stands for, or the value itself. A row that only opens
 * the settings names none.
 */
export const optionModel = (value: string): string => {
	const local = parseLocalModelKey(value);
	if (local) return local.model;
	if (value === LOCAL_SETUP_VALUE) return "";
	if (!value.startsWith(LOCAL_SETUP_VALUE + SETUP_BAR)) return value;
	const rest = value.slice(LOCAL_SETUP_VALUE.length + 1);
	const bar = rest.indexOf(SETUP_BAR);
	return bar < 0 ? "" : rest.slice(bar + 1);
};

/**
 * The model picker's rows that `query` matches, by name or by model, as
 * `matchesModel` says. The last row, which opens the Local models
 * settings, stays.
 */
export const filterModelOptions = (options: readonly Option[], query: string): Option[] =>
	options.filter(
		(o) =>
			o.value === LOCAL_SETUP_VALUE ||
			matchesModel({ label: o.label, id: optionModel(o.value) }, query)
	);

/** How many models a model picker's rows list, for `showsModelSearch`: all but the setup row. */
export const modelOptionCount = (options: readonly Option[]): number =>
	options.filter((o) => o.value !== LOCAL_SETUP_VALUE).length;

/**
 * The row under a Mac's models when its switch keeps this device's agents
 * off them. Picking it opens that Mac's Local models settings, where the
 * switch is.
 */
const turnOnOption = (view: ViewState, mac: DeviceId, group: string): Option => ({
	value: [LOCAL_SETUP_VALUE, mac].join(SETUP_BAR),
	label: t("Open {{name}}'s Local models settings…", { name: deviceName(view, mac) }),
	detail: t('Turn on "{{switch}}" there', { switch: linkedDevicesLabel() }),
	warn: true,
	group,
});

/** The row that opens the Local models settings. */
export const localSetupOption = (): Option => ({
	value: LOCAL_SETUP_VALUE,
	label: t("Set up local models…"),
});

/**
 * The Local entry in the agent picker. With a local model ready it picks
 * the device's local agent, as `localAccountOption` says; without one it
 * still shows, and picking it opens the device's Local models settings.
 */
export const localKindOption = (
	view: ViewState,
	device: DeviceId,
	selected: Account["id"] | null = null
): Option => {
	const option = localAccountOption(view, device, selected);
	if (option && !option.disabled) return option;
	const support = localModelsOf(view, device)?.runtime.support;
	return {
		value: LOCAL_SETUP_VALUE,
		label: kindName("local"),
		detail: !runsLocalModels(view, device)
			? t("{{name}}'s maxd is too old for this. Update it first.", {
					name: deviceName(view, device),
				})
			: support === "managed"
				? t("Download a model in Settings first")
				: t("Add a server URL in Settings first"),
		warn: true,
	};
};

/**
 * The agent picker's entries for a device: its logins, then Local, which
 * always shows.
 */
export const agentOptions = (
	view: ViewState,
	device: DeviceId,
	now: number,
	selected: Account["id"] | null = null
): Option[] => {
	const local = (value: string): boolean => {
		const account = view.tables.account.get(value);
		return value === LOCAL_SETUP_VALUE || (account !== undefined && isLocalAccount(account));
	};
	return [
		...accountOptions(view, device, now, selected).filter((o) => !local(o.value)),
		localKindOption(view, device, selected),
	];
};

/**
 * The catalog models a device could download but hasn't yet, under the
 * group its managed server's models show in. Picking one opens that
 * device's Local models settings.
 */
const notDownloaded = (view: ViewState, device: DeviceId): Option[] => {
	const managed = serversOf(view, device).find((s) => s.kind.type === "managed");
	const group = managed ? serverGroup(view, managed) : deviceGroup(view, device);
	return catalogRows(view, device)
		.filter((row) => row.onDisk?.state.state !== "ready")
		.filter((row) => !managed?.models.some((m) => m.catalog?.model === row.model.id))
		.map((row) => ({
			value: [LOCAL_SETUP_VALUE, device, row.model.id].join(SETUP_BAR),
			label: row.model.name,
			detail: row.onDisk
				? fileStateText(row.onDisk)
				: t("{{size}} download", { size: formatFileSize(row.file.bytes) }),
			tag: t("Not downloaded"),
			group,
		}));
};

/**
 * The model picker's entries for a local agent on a device: each server's
 * models under "This Mac" or the server's name, with a fit tag where it's
 * known; the catalog models not downloaded yet; and a row that opens the
 * Local models settings. A Mac whose switch keeps this device's agents off
 * its models shows them turned off, with a row that opens its settings. `modelChoice` reads a model's value back, and
 * `opensLocalSetup` the others.
 */
export const localPickerOptions = (view: ViewState, device: DeviceId): Option[] => {
	const listed = localModelOptions(view, device);
	const accounts = localAccountsOn(view, device);
	const models = accounts.flatMap((account): Option[] => {
		const server = serverOf(view, account);
		if (!server) return [];
		const group = serverHeading(view, server);
		const prefix = localModelKey(account.id, "");
		const rows = listed
			.filter((option) => option.value.startsWith(prefix))
			.map((option) => {
				const id = parseLocalModelKey(option.value)?.model;
				const model = server.models.find((m) => m.id === id);
				const tag = model && !option.disabled ? fitTag(view, server, model) : null;
				return {
					...option,
					group,
					...(tag ? { tag: tag.label, tone: tag.tone } : {}),
				};
			});
		return sharingOff(view, device, server)
			? [...rows, turnOnOption(view, server.device, group)]
			: rows;
	});
	// The catalogs to offer: the device's own, and those of the Macs whose
	// servers its local agents reach.
	const catalogs = [
		...new Set([
			device,
			...accounts
				.map((account) => serverOf(view, account))
				.filter(
					(server) => server?.kind.type === "managed" && !sharingOff(view, device, server)
				)
				.map((server) => server!.device),
		]),
	];
	const rows = [...models, ...catalogs.flatMap((d) => notDownloaded(view, d))];
	// The models not downloaded join their server's group, so a server at a
	// URL listed between them doesn't split it.
	const groups = [...new Set(rows.map((row) => row.group))];
	return [...groups.flatMap((g) => rows.filter((row) => row.group === g)), localSetupOption()];
};
