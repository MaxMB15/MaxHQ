// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the Local models settings page shows and offers: the devices to
// pick from and the one it opens on, the device's hardware in one line, the
// default model's choices, what each catalog row can do with its file, the
// managed server's state, a server's last test, the form that adds a server
// URL, and the choices for the settings. The managed server's port and key
// stay with `maxd`, so nothing here names them.

import { formatNumber, t } from "../i18n";

import {
	DEFAULT_CATALOG_MODEL,
	LOCAL_MODEL_DEFAULTS,
	localModelsOf,
	localPicksOn,
	serverModelName,
	type LocalPick,
	type LocalSettings,
} from "./models";
import { serverGroup } from "./picker";
import { type CatalogRow, runsLocalModels, serversOf, serverUsers } from "./select";
import {
	downloadPercent,
	fileStateText,
	flavorName,
	formatFileSize,
	formatMemory,
	loadText,
	notEncrypted,
	serverProblemText,
	urlParts,
} from "./wording";
import type {
	DeviceHardware,
	DeviceId,
	KvCacheType,
	LocalModels,
	ModelAction,
	ModelServer,
	ModelServerId,
	ServerModel,
} from "../bindings";
import type { ActionResult } from "../data/source";
import type { Option } from "../new-agent/spec";
import { isThisMac } from "../settings/app-keys";
import type { Tone } from "../tasks/format";
import { all, type ViewState } from "../view/replica";
import { deviceName } from "../view/select";

// Devices

/** The devices the page offers, this Mac first and then by name. */
export const localDevices = (view: ViewState): DeviceId[] => {
	const devices = new Set<DeviceId>();
	for (const link of all(view, "device_link")) {
		if (link.device && !link.claim) devices.add(link.device);
	}
	return [...devices].sort(
		(a, b) =>
			Number(isThisMac(view, b)) - Number(isThisMac(view, a)) ||
			deviceName(view, a).localeCompare(deviceName(view, b)) ||
			a.localeCompare(b)
	);
};

/** The devices to pick from, by name, with this Mac marked. */
export const localDeviceOptions = (view: ViewState): Option[] =>
	localDevices(view).map((device) => ({
		value: device,
		label: deviceName(view, device),
		...(isThisMac(view, device) ? { detail: t("This Mac") } : {}),
	}));

/**
 * The device the page opens on when none is asked for: this Mac, else the
 * first device that reports local models, else the first device.
 */
export const defaultLocalDevice = (view: ViewState): DeviceId | null => {
	const devices = localDevices(view);
	return (
		devices.find((d) => isThisMac(view, d)) ??
		devices.find((d) => localModelsOf(view, d) !== undefined) ??
		devices[0] ??
		null
	);
};

/** The device the page shows: the one asked for while the view has it, else the default. */
export const pageDevice = (
	view: ViewState,
	wanted: DeviceId | null | undefined
): DeviceId | null =>
	wanted && localDevices(view).includes(wanted) ? wanted : defaultLocalDevice(view);

/** Whether the page can say anything about a device's local models yet. */
export type DeviceState =
	/** Its `maxd` is too old for local models. */
	| { readonly state: "too_old" }
	/** It runs them but hasn't reported its hardware yet. */
	| { readonly state: "waiting" }
	| { readonly state: "ready"; readonly local: LocalModels };

/** Where a device's local models stand for the page. */
export const deviceState = (view: ViewState, device: DeviceId): DeviceState => {
	if (!runsLocalModels(view, device)) return { state: "too_old" };
	const local = localModelsOf(view, device);
	return local ? { state: "ready", local } : { state: "waiting" };
};

/** A device's chip, GPU and memory in one line, such as "Apple M4 Max, 32-core GPU, 36 GB memory". */
export const hardwareText = (hardware: DeviceHardware): string =>
	[
		hardware.chip,
		hardware.gpu_cores
			? t("{{cores}}-core GPU", { cores: formatNumber(hardware.gpu_cores) })
			: null,
		t("{{memory}} memory", { memory: formatMemory(hardware.ram) }),
	]
		.filter((part): part is string => part !== null && part !== "")
		.join(", ");

/** The free space on the disk that holds the models, such as "227.6 GB free". */
export const diskFreeText = (local: Pick<LocalModels, "disk_free">): string =>
	t("{{size}} free", { size: formatFileSize(local.disk_free) });

// Catalog rows

/** A button on a catalog row. */
export type RowButton = "download" | "pause" | "resume" | "retry" | "load" | "unload" | "delete";

/** A model on the managed server, with that server. */
export type Served = { readonly server: ModelServer; readonly model: ServerModel };

/** What a catalog row shows beside the model and offers to do with its file. */
export type ModelRowView = {
	/** Where the file or the load stands, such as "Downloading, 42%", or null before a download. */
	readonly status: string | null;
	/** Whether `status` says something failed. */
	readonly error: boolean;
	/**
	 * How far the download is, 0 to 100, while it's on its way or paused; or
	 * how far the model's load into memory is, when `loading` says so.
	 */
	readonly percent: number | null;
	/** Whether `percent` is the model's load into memory, not its download. */
	readonly loading: boolean;
	/** Whether the file is being checked, or the model loaded, so the row waits. */
	readonly busy: boolean;
	/** Before a download: its size and the free space, and whether that's too little. */
	readonly disk: { readonly text: string; readonly short: boolean } | null;
	/** The buttons, in order. */
	readonly buttons: readonly RowButton[];
	/** The managed server's model for the file, once the server lists it. */
	readonly served: Served | null;
};

/** The managed server on a device's model, for a catalog row's file. */
export const servedModel = (view: ViewState, device: DeviceId, row: CatalogRow): Served | null => {
	const server = serversOf(view, device).find((s) => s.kind.type === "managed");
	const model = server?.models.find(
		(m) => m.catalog?.model === row.model.id && m.catalog.file === row.file.name
	);
	return server && model ? { server, model } : null;
};

const row = (view: Partial<ModelRowView>): ModelRowView => ({
	status: null,
	error: false,
	percent: null,
	loading: false,
	busy: false,
	disk: null,
	buttons: [],
	served: null,
	...view,
});

/**
 * What a catalog row shows and offers. Before a download it offers
 * Download, with the size and the free space; a download in progress
 * offers Pause and Delete, a paused one Resume and Delete, a failed one
 * Retry and Delete, and one being checked nothing. A downloaded file the
 * managed server lists offers Load and Delete, or Unload once it's loaded.
 */
export const modelRow = (
	view: ViewState,
	device: DeviceId,
	catalogRow: CatalogRow
): ModelRowView => {
	const file = catalogRow.onDisk;
	if (!file) {
		const free = localModelsOf(view, device)?.disk_free ?? null;
		const size = formatFileSize(catalogRow.file.bytes);
		const short = free !== null && free < catalogRow.file.bytes;
		return row({
			disk: {
				text:
					free === null
						? t("{{size}} download", { size })
						: short
							? t("Needs {{size}}, but only {{free}} is free", {
									size,
									free: formatFileSize(free),
								})
							: t("{{size}} download, {{free}} free", {
									size,
									free: formatFileSize(free),
								}),
				short,
			},
			buttons: ["download"],
		});
	}
	switch (file.state.state) {
		case "downloading":
			return file.state.phase === "verifying"
				? row({ status: fileStateText(file), percent: 100, busy: true })
				: row({
						status: fileStateText(file),
						percent: downloadPercent(file),
						buttons: ["pause", "delete"],
					});
		case "paused":
			return row({
				status: fileStateText(file),
				percent: downloadPercent(file),
				buttons: ["resume", "delete"],
			});
		case "failed":
			return row({ status: fileStateText(file), error: true, buttons: ["retry", "delete"] });
		case "ready": {
			const served = servedModel(view, device, catalogRow);
			if (!served) return row({ status: fileStateText(file), buttons: ["delete"] });
			const load = served.model.load ?? { state: "unloaded" as const };
			const status = loadText(load);
			switch (load.state) {
				case "unloaded":
					return row({ status, served, buttons: ["load", "delete"] });
				case "loading":
					return row({
						status,
						served,
						busy: true,
						percent: load.percent ?? null,
						loading: true,
					});
				case "loaded":
					return row({ status, served, buttons: ["unload"] });
				case "failed":
					return row({ status, served, error: true, buttons: ["load", "delete"] });
			}
		}
	}
};

/** What a row's button says. */
export const rowButtonLabel = (button: RowButton): string => {
	switch (button) {
		case "download":
			return t("Download");
		case "pause":
			return t("Pause");
		case "resume":
			return t("Resume");
		case "retry":
			return t("Retry");
		case "load":
			return t("Load model");
		case "unload":
			return t("Unload model");
		case "delete":
			return t("Delete");
	}
};

/** What a row's button sends, or null when the server doesn't list the model yet. */
export const rowModelAction = (
	catalogRow: CatalogRow,
	shown: Pick<ModelRowView, "served">,
	button: RowButton
): ModelAction | null => {
	const file = { model: catalogRow.model.id, file: catalogRow.file.name };
	switch (button) {
		case "download":
			return { action: "download", ...file };
		case "pause":
			return { action: "pause_download", ...file };
		case "resume":
		case "retry":
			return { action: "resume_download", ...file };
		case "delete":
			return { action: "delete_model", ...file };
		case "load":
		case "unload":
			return shown.served
				? { action: button, server: shown.served.server.id, model: shown.served.model.id }
				: null;
	}
};

/** What Delete asks before it deletes, naming the space it frees. */
export const deleteQuestion = (catalogRow: CatalogRow): string => {
	const file = catalogRow.onDisk;
	return file && file.state.state !== "ready"
		? t("Stop the download and delete the {{size}} downloaded so far?", {
				size: formatFileSize(file.bytes_done),
			})
		: t("Delete {{model}} and free {{size}}?", {
				model: catalogRow.model.name,
				size: formatFileSize(catalogRow.file.bytes),
			});
};

// Servers

/** A model on the managed server and where its load stands. */
export type ActiveModel = { readonly name: string; readonly load: string; readonly tone: Tone };

/** What the managed server's row says. Its port and key never show. */
export type ManagedServerView = {
	/** The server's install: its version, an update in progress, or why it failed. */
	readonly state: string;
	readonly error: boolean;
	/** The model it has loaded or is loading, or null with none. */
	readonly model: ActiveModel | null;
	/** The other devices whose Local agents use it over the link, or null with none. */
	readonly users: string | null;
	/** Why it can't run a Local agent, or null when it can. */
	readonly problem: string | null;
};

const LOAD_TONE: Record<"loading" | "loaded" | "failed", Tone> = {
	loading: "info",
	loaded: "good",
	failed: "bad",
};

/** What the managed server's row says: its install, its loaded model and who else uses it. */
export const managedServerView = (view: ViewState, server: ModelServer): ManagedServerView => {
	const runtime = localModelsOf(view, server.device)?.runtime;
	const install = runtime?.state;
	const state =
		install?.type === "running"
			? t("Setting up: {{message}}", { message: install.message })
			: install?.type === "failed"
				? t("Couldn't set it up: {{reason}}", { reason: install.reason })
				: runtime?.version
					? `${flavorName("llama_cpp")} ${runtime.version}`
					: t("Not set up yet");
	const active =
		server.models.find((m) => m.load?.state === "loading") ??
		server.models.find((m) => m.load?.state === "loaded") ??
		server.models.find((m) => m.load?.state === "failed");
	const load = active?.load;
	const users = serverUsers(view, server).map((d) => deviceName(view, d));
	return {
		state,
		error: install?.type === "failed",
		model:
			active && load && load.state !== "unloaded"
				? {
						name: serverModelName(view, server, active),
						load: loadText(load) ?? "",
						tone: LOAD_TONE[load.state],
					}
				: null,
		users:
			users.length > 0
				? t("Local agents on {{devices}} use it too", { devices: users.join(", ") })
				: null,
		problem: serverProblemText(server),
	};
};

/** A server's last test in words, and whether it passed; null before any test. */
export type ProbeSummary = { readonly text: string; readonly ok: boolean | null };

/** What a server's last test found: that it works, and its context, or why it can't run a Local agent. */
export const probeSummary = (server: Pick<ModelServer, "probe">): ProbeSummary => {
	const probe = server.probe;
	if (!probe) return { text: t("Not tested yet"), ok: null };
	const problem = serverProblemText(server);
	if (problem) return { text: problem, ok: false };
	return {
		text: probe.context
			? t("Works with Local agents, {{context}} tokens of context", {
					context: formatNumber(probe.context),
				})
			: t("Works with Local agents"),
		ok: true,
	};
};

/** What Remove asks before it removes a server URL. */
export const removeServerQuestion = (): string =>
	t("Local agents here can't use it after this, and its token is deleted.");

// The form that adds a server URL

/** Why a typed server URL won't do, or null when it will or nothing's typed. */
export const serverUrlProblem = (url: string): string | null => {
	const typed = url.trim();
	if (!typed) return null;
	const parts = urlParts(typed);
	return parts && (parts.scheme === "http" || parts.scheme === "https")
		? null
		: t("Type a URL that starts with http:// or https://, such as http://devbox:11434.");
};

/** Whether the form has a URL to add. */
export const canAddServer = (url: string): boolean =>
	url.trim() !== "" && serverUrlProblem(url) === null;

/** Whether a typed URL sends plain HTTP to another machine. */
export const typedNotEncrypted = (url: string): boolean =>
	notEncrypted({ base_url: url.trim() || null });

/** What plain HTTP to another machine risks, and that adding it still works. */
export const notEncryptedDetail = (): string =>
	t(
		"Plain http to another machine: anyone on the network in between can read your prompts, your code and the token. Use https, or reach it over SSH or a VPN, to keep it private. You can still add it."
	);

/** The name a new server gets: the one typed, else the URL's host. */
export const serverLabelFor = (label: string, url: string): string =>
	label.trim() || urlParts(url.trim())?.host || url.trim();

/** What the form sends to add a server, with the token when one was typed. */
export const addServerAction = (label: string, url: string, token: string): ModelAction => ({
	action: "add_server",
	label: serverLabelFor(label, url),
	url: url.trim(),
	token: token.trim() || null,
});

/** The server an add made, from its reply. */
export const addedServerOf = (result: ActionResult): ModelServerId | null =>
	result.status === "ok" && result.data.status === "model_server_added"
		? result.data.server
		: null;

/**
 * The form's Test: what answers at the typed URL, with the token typed.
 * `maxd` keeps nothing of either; only Add saves the server.
 */
export const probeServerAction = (url: string, token: string): ModelAction => ({
	action: "probe_server",
	url: url.trim(),
	token: token.trim() || null,
});

/** What the form's Test found at a URL. */
export type ProbeFindings = {
	/** What kind of server answered, such as "Ollama", or null when nothing did. */
	readonly flavor: string | null;
	/** Whether it can run a Local agent, with its context, or why not. */
	readonly summary: ProbeSummary;
	/** The models it offers, such as "Offers gpt-oss-20b, Qwen3", or null when nothing answered. */
	readonly models: string | null;
};

/** What a Test found, from its reply; null for another reply. */
export const probeFindings = (
	view: ViewState,
	device: DeviceId,
	result: ActionResult
): ProbeFindings | null => {
	if (result.status !== "ok" || result.data.status !== "server_probed") return null;
	const { flavor, probe, models } = result.data;
	const answered = probe.error === null;
	const names = models.map((m) => serverModelName(view, { device }, m));
	return {
		flavor: answered ? flavorName(flavor) : null,
		summary: probeSummary({ probe }),
		models: !answered
			? null
			: names.length > 0
				? t("Offers {{models}}", { models: names.join(", ") })
				: t("This server lists no models"),
	};
};

// Default model

const PICK_BAR = "|";

/** The value that keeps the default automatic. */
export const AUTO_DEFAULT = "";

/** The picker value for a server's model. */
export const pickValue = (server: ModelServerId, model: string): string =>
	`${server}${PICK_BAR}${model}`;

/** The default model's picker value for the settings. */
export const defaultModelValue = (settings: Pick<LocalSettings, "default_model">): string =>
	settings.default_model
		? pickValue(settings.default_model.server, settings.default_model.model)
		: AUTO_DEFAULT;

/** What a device picks with no default set: the catalog's default model, else the first. */
const automaticPick = (picks: readonly LocalPick[]): LocalPick | undefined =>
	picks.find((p) => p.model.catalog?.model === DEFAULT_CATALOG_MODEL) ?? picks[0];

/**
 * The default model's choices: Automatic, then every model the device's
 * Local agents can start on, under its server. A saved default that's gone
 * shows as such, so the picker still names it.
 */
export const defaultModelOptions = (
	view: ViewState,
	device: DeviceId,
	settings: Pick<LocalSettings, "default_model">
): Option[] => {
	const picks = localPicksOn(view, device);
	const auto = automaticPick(picks);
	const options: Option[] = [
		{
			value: AUTO_DEFAULT,
			label: t("Automatic"),
			detail: auto ? serverModelName(view, auto.server, auto.model) : t("No model ready yet"),
		},
	];
	const seen = new Set<string>([AUTO_DEFAULT]);
	for (const pick of picks) {
		const value = pickValue(pick.server.id, pick.model.id);
		if (seen.has(value)) continue;
		seen.add(value);
		options.push({
			value,
			label: serverModelName(view, pick.server, pick.model),
			group: serverGroup(view, pick.server),
		});
	}
	const saved = defaultModelValue(settings);
	if (!seen.has(saved) && settings.default_model) {
		options.push({
			value: saved,
			label: settings.default_model.model,
			detail: t("Not available now"),
			warn: true,
		});
	}
	return options;
};

/** The model a picked value names, or null for Automatic or one that's gone. */
export const pickOf = (view: ViewState, device: DeviceId, value: string): LocalPick | null =>
	value === AUTO_DEFAULT
		? null
		: (localPicksOn(view, device).find((p) => pickValue(p.server.id, p.model.id) === value) ??
			null);

// Settings

/** How the managed server can store its KV cache. */
export const kvTypeOptions = (): Option[] => [
	{
		value: "f16" satisfies KvCacheType,
		label: t("Full size (f16)"),
		detail: t("The default, with the best quality"),
	},
	{
		value: "q8_0" satisfies KvCacheType,
		label: t("Half size (q8_0)"),
		detail: t("Half the memory for the context, with nearly the same quality"),
	},
];

/** The context sizes to pick from, in tokens. */
export const CONTEXT_SIZES: readonly number[] = [16_384, 32_768, 65_536, 131_072, 262_144];

/** The context sizes to pick from, with `current` among them. */
export const contextOptions = (current: number): Option[] =>
	[...new Set([...CONTEXT_SIZES, current])]
		.sort((a, b) => a - b)
		.map((size) => ({
			value: String(size),
			label: t("{{size}} tokens", { size: formatNumber(size) }),
			...(size === LOCAL_MODEL_DEFAULTS.context ? { detail: t("The default") } : {}),
		}));

/** The most minutes the idle unload takes. */
export const MAX_IDLE_MINUTES = 1440;

/** The minutes typed for the idle unload, or why they won't do. */
export const parseIdleMinutes = (
	text: string
):
	| { readonly ok: true; readonly minutes: number }
	| { readonly ok: false; readonly reason: string } => {
	const typed = text.trim();
	const minutes = /^\d{1,4}$/.test(typed) ? Number(typed) : NaN;
	return minutes >= 0 && minutes <= MAX_IDLE_MINUTES
		? { ok: true, minutes }
		: {
				ok: false,
				reason: t("Type a whole number of minutes from 0 to {{max}}.", {
					max: formatNumber(MAX_IDLE_MINUTES),
				}),
			};
};
