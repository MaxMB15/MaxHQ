// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the views say about local models: fit, speed, the GPU memory limit,
// where a model file or a load stands, and the notes on a server row.

import { formatNumber, t } from "../i18n";

import { GIB, type Fit, type Range } from "./fit";
import type {
	DeviceHardware,
	ModelFile,
	ModelLoad,
	ModelScore,
	ModelServer,
	RuntimeSupport,
	ServerFlavor,
} from "../bindings";

/** A fit as its badge reads. */
export const fitLabel = (fit: Fit): string => {
	switch (fit) {
		case "fits":
			return t("Fits");
		case "tight":
			return t("Tight");
		case "too_large":
			return t("Too large");
	}
};

/** What a fit means, for the badge's tooltip. */
export const fitDetail = (fit: Fit): string => {
	switch (fit) {
		case "fits":
			return t("Fits in the memory the GPU can use, with room to spare.");
		case "tight":
			return t("Fits, but leaves little memory for your other apps.");
		case "too_large":
			return t("Needs more memory than the GPU can use on this device.");
	}
};

const oneDecimal = (value: number): string =>
	formatNumber(value, {
		minimumFractionDigits: value < 10 ? 1 : 0,
		maximumFractionDigits: value < 10 ? 1 : 0,
	});

/**
 * Memory, such as RAM or what a model needs, in the binary gigabytes macOS
 * shows, such as "36 GB".
 */
export const formatMemory = (bytes: number): string =>
	t("{{value}} GB", { value: oneDecimal(bytes / GIB) });

/** A file's size, in the decimal gigabytes model hubs show, such as "22.4 GB". */
export const formatFileSize = (bytes: number): string =>
	bytes >= 1e9
		? t("{{value}} GB", { value: formatNumber(bytes / 1e9, { maximumFractionDigits: 1 }) })
		: t("{{value}} MB", { value: formatNumber(Math.round(bytes / 1e6)) });

/** A speed estimate, such as "60-90 tokens/s". */
export const speedText = (range: Range): string =>
	t("{{low}}-{{high}} tokens/s", {
		low: formatNumber(Math.round(range.low)),
		high: formatNumber(Math.round(range.high)),
	});

/** A cold first turn's estimate, such as "First turn about 20-40 s". */
export const coldTurnText = (range: Range): string =>
	range.high < 120
		? t("First turn about {{low}}-{{high}} s", {
				low: formatNumber(Math.round(range.low)),
				high: formatNumber(Math.round(range.high)),
			})
		: t("First turn about {{low}}-{{high}} min", {
				low: formatNumber(Math.max(1, Math.round(range.low / 60))),
				high: formatNumber(Math.max(1, Math.round(range.high / 60))),
			});

/** Why the speeds are estimates, for their tooltip. */
export const estimateNote = (): string =>
	t(
		"Estimated from the chip's memory bandwidth and GPU. A first turn reads Claude Code's prompt of about 25,000 tokens; later turns reuse it and take a few seconds."
	);

/**
 * Where a device's GPU memory budget comes from, and that MaxHQ leaves it
 * alone. Null for a device without a GPU.
 */
export const gpuLimitText = (hardware: DeviceHardware): string | null => {
	const gpu = hardware.gpu;
	if (!gpu) return null;
	const budget = formatMemory(gpu.budget);
	const ram = formatMemory(hardware.ram);
	switch (gpu.source) {
		case "wired_limit":
			return t(
				"The GPU can use {{budget}} of this Mac's {{ram}}, a limit someone set with iogpu.wired_limit_mb. MaxHQ leaves it as it is.",
				{ budget, ram }
			);
		case "metal":
		case "estimate":
			return t(
				"macOS lets the GPU use about {{budget}} of this Mac's {{ram}}, and keeps the rest for your apps. A model has to fit in that. MaxHQ doesn't raise the limit: raising it can leave macOS short of memory and freeze the Mac.",
				{ budget, ram }
			);
	}
};

/** Why a device can't run a managed server, or null when it can. */
export const runtimeSupportText = (support: RuntimeSupport): string | null => {
	switch (support) {
		case "managed":
			return null;
		case "no_gpu":
			return t(
				"Too slow for agents: this device has no GPU MaxHQ can use. Add a server URL to use a model running elsewhere, such as on a Mac."
			);
		case "url_only":
			return t("MaxHQ doesn't run models on this device yet. Add a server URL instead.");
	}
};

/** The short form of `runtimeSupportText` for a device without a GPU. */
export const tooSlowLabel = (): string => t("Too slow for agents");

/** What a server of a flavor is called. */
export const flavorName = (flavor: ServerFlavor): string => {
	switch (flavor) {
		case "llama_cpp":
			return "llama.cpp";
		case "ollama":
			return "Ollama";
		case "lm_studio":
			return "LM Studio";
		case "vllm":
			return "vLLM";
		case "other":
			return t("Model server");
	}
};

/** Whether a host name or address is this machine's own. */
const isLoopback = (host: string): boolean => {
	const bare = host.replace(/^\[|\]$/g, "").toLowerCase();
	return (
		bare === "localhost" ||
		bare.endsWith(".localhost") ||
		bare === "::1" ||
		/^127(\.\d{1,3}){3}$/.test(bare)
	);
};

/** A server URL's scheme, host and port, all lower case. */
export type UrlParts = { readonly scheme: string; readonly host: string; readonly port: string };

/** The scheme, host and port of a URL, or null when it isn't one. */
export const urlParts = (url: string): UrlParts | null => {
	const match =
		/^([a-z][a-z0-9+.-]*):\/\/(?:[^@/?#]*@)?(\[[^\]]*\]|[^:/?#]+)(?::(\d+))?(?:[/?#]|$)/i.exec(
			url.trim()
		);
	if (!match) return null;
	return {
		scheme: match[1]!.toLowerCase(),
		host: match[2]!.toLowerCase(),
		port: match[3] ?? "",
	};
};

/**
 * Whether a server's traffic crosses the network unencrypted: plain HTTP to
 * an address other than this machine's own. It still works; the row says so.
 */
export const notEncrypted = (server: Pick<ModelServer, "base_url">): boolean => {
	const url = server.base_url ? urlParts(server.base_url) : null;
	return url !== null && url.scheme === "http" && !isLoopback(url.host);
};

/** The note on a server row whose traffic isn't encrypted. */
export const notEncryptedText = (): string => t("Not encrypted on the network");

/** Why a server can't run a Local agent, or null when it can or before a probe. */
export const serverProblemText = (server: Pick<ModelServer, "probe">): string | null => {
	const probe = server.probe;
	if (!probe) return null;
	if (probe.error) return probe.error;
	if (probe.apis.messages !== true)
		return t(
			"This server doesn't answer the Anthropic Messages API (/v1/messages), so it can't run a Local agent."
		);
	return null;
};

/** Where a model stands in memory, such as "Loading model". */
export const loadText = (load: ModelLoad | null): string | null => {
	if (!load) return null;
	switch (load.state) {
		case "unloaded":
			return t("Not loaded");
		case "loading":
			return t("Loading model");
		case "loaded":
			return t("Loaded");
		case "failed":
			return t("Couldn't load: {{reason}}", { reason: load.reason });
	}
};

/** How far a download is, from 0 to 100. */
export const downloadPercent = (file: Pick<ModelFile, "bytes_done" | "bytes_total">): number =>
	file.bytes_total > 0 ? Math.min(100, (file.bytes_done / file.bytes_total) * 100) : 0;

/** Where a model file stands, such as "Downloading, 42%". */
export const fileStateText = (file: ModelFile): string => {
	switch (file.state.state) {
		case "downloading":
			return file.state.phase === "verifying"
				? t("Checking the download")
				: t("Downloading, {{percent}}%", {
						percent: formatNumber(Math.floor(downloadPercent(file))),
					});
		case "paused":
			return t("Paused, {{percent}}%", {
				percent: formatNumber(Math.floor(downloadPercent(file))),
			});
		case "ready":
			return t("Downloaded");
		case "failed":
			return t("Download failed: {{reason}}", { reason: file.state.reason });
	}
};

/** Who reported a score and when, such as "Reported by the vendor, 2026-04-16". */
export const scoreSourceText = (score: Pick<ModelScore, "source" | "date">): string =>
	score.source === "vendor"
		? t("Reported by the vendor, {{date}}", { date: score.date })
		: t("Measured independently, {{date}}", { date: score.date });

/** A local agent as it's named everywhere: "Local · <model>". */
export const localAgentLabel = (model: string | null): string =>
	model ? t("Local · {{model}}", { model }) : t("Local");
