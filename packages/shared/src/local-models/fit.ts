// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Whether a model fits a device's GPU memory, and how fast it should run.
// The memory a model needs is its weights, its KV cache for the context
// (once per slot) and about 1 GiB of buffers, against the memory macOS lets
// the GPU use. Speed comes from the chip: writing is bound by memory
// bandwidth over the bytes of weights each token reads, reading the prompt
// by the GPU's compute over the active parameters. These are estimates for
// picking a model, not promises; the footer shows the real speed.

import type { CatalogFile, CatalogModel, DeviceHardware, KvCacheType, KvShape } from "../bindings";

/** Whether a model fits: with room to spare, just, or not at all. */
export type Fit = "fits" | "tight" | "too_large";

/** A low and a high estimate. */
export type Range = { readonly low: number; readonly high: number };

/** One GiB, in bytes. */
export const GIB = 1024 ** 3;

/** The buffers a loaded model needs beside its weights and KV cache. */
export const OVERHEAD_BYTES = GIB;

/** A model fits with room to spare below this share of the GPU budget. */
export const ROOMY_SHARE = 0.9;

/** The prompt Claude Code sends on a first turn, in tokens. */
export const CLAUDE_CODE_PROMPT_TOKENS = 25_000;

/** Bytes per cached element for each KV cache type. q8_0 packs 32 in 34. */
const KV_ELEMENT_BYTES: Record<KvCacheType, number> = { f16: 2, q8_0: 34 / 32 };

/**
 * The bytes a KV cache holds for `context` tokens: keys and values for each
 * layer, where a sliding-window layer holds no more than its window.
 */
export const kvBytes = (kv: KvShape, context: number, type: KvCacheType): number => {
	const perLayerToken = 2 * kv.kv_heads * kv.head_dim * KV_ELEMENT_BYTES[type];
	const full = kv.full_layers * context;
	const window = (kv.swa_layers ?? 0) * Math.min(kv.swa_window ?? 0, context);
	return perLayerToken * (full + window);
};

/** What a load asks for. */
export type LoadShape = {
	/** The model file's size in bytes. */
	readonly bytes: number;
	/** The model's KV shape. */
	readonly kv: KvShape;
	/** The context size of each slot, in tokens. */
	readonly context: number;
	/** How the KV cache is stored. */
	readonly kvType: KvCacheType;
	/** How many requests it serves at once, each with its own KV cache. */
	readonly slots?: number;
};

/** The memory a load needs: weights, the KV cache per slot, and buffers. */
export const memoryNeeded = (load: LoadShape): number =>
	load.bytes + kvBytes(load.kv, load.context, load.kvType) * (load.slots ?? 1) + OVERHEAD_BYTES;

/** Whether `need` bytes fit a GPU budget of `budget` bytes. */
export const fitOf = (need: number, budget: number): Fit =>
	need <= budget * ROOMY_SHARE ? "fits" : need <= budget ? "tight" : "too_large";

/**
 * macOS's default share of memory for the GPU, for when `maxd` couldn't read
 * the real figure: about two thirds on small Macs and three quarters on the
 * rest.
 */
export const estimatedGpuBudget = (ram: number): number =>
	Math.floor(ram <= 16 * GIB ? (ram * 2) / 3 : (ram * 3) / 4);

/** The GPU budget in bytes, or null for a device without a GPU `maxd` can use. */
export const gpuBudget = (hardware: DeviceHardware): number | null => hardware.gpu?.budget ?? null;

/** An Apple chip, as far as speed goes. */
export type Chip = {
	/** Its generation, such as 4 for M4. */
	readonly generation: number;
	/** Its tier. */
	readonly tier: "base" | "pro" | "max" | "ultra";
	/** Its GPU cores, as reported or the tier's smallest. */
	readonly cores: number;
	/** Its memory bandwidth in bytes per second. */
	readonly bandwidth: number;
	/** Its GPU's half-precision throughput in floating-point operations per second. */
	readonly flops: number;
};

type Tier = Chip["tier"];

/**
 * Memory bandwidth in GB/s per generation and tier. A tier with two figures
 * has a cut-down GPU below the core count given, as the M3 Max and M4 Max do.
 */
const BANDWIDTH: Record<number, Partial<Record<Tier, readonly [number, number?, number?]>>> = {
	1: { base: [68], pro: [200], max: [400], ultra: [800] },
	2: { base: [100], pro: [200], max: [400], ultra: [800] },
	3: { base: [100], pro: [150], max: [300, 40, 400], ultra: [819] },
	4: { base: [120], pro: [273], max: [410, 40, 546] },
	5: { base: [153] },
};

/** Half-precision TFLOPS per GPU core, per generation. */
const TFLOPS_PER_CORE: Record<number, number> = { 1: 0.325, 2: 0.358, 3: 0.41, 4: 0.43, 5: 0.42 };

/** The smallest GPU core count of each generation and tier, when the device doesn't say. */
const SMALLEST_CORES: Record<number, Partial<Record<Tier, number>>> = {
	1: { base: 7, pro: 14, max: 24, ultra: 48 },
	2: { base: 8, pro: 16, max: 30, ultra: 60 },
	3: { base: 8, pro: 14, max: 30, ultra: 60 },
	4: { base: 8, pro: 16, max: 32 },
	5: { base: 8 },
};

/** The chip a device has, or null when it isn't an Apple chip this build knows. */
export const chipOf = (hardware: Pick<DeviceHardware, "chip" | "gpu_cores">): Chip | null => {
	const match = /\bM(\d+)(?:\s+(Pro|Max|Ultra))?\b/i.exec(hardware.chip ?? "");
	if (!match) return null;
	const generation = Number(match[1]);
	const tier = (match[2]?.toLowerCase() ?? "base") as Tier;
	const rates = BANDWIDTH[generation]?.[tier];
	const perCore = TFLOPS_PER_CORE[generation];
	const cores = hardware.gpu_cores ?? SMALLEST_CORES[generation]?.[tier];
	if (!rates || perCore === undefined || cores === undefined) return null;
	const [slow, from, fast] = rates;
	const gbps = from !== undefined && fast !== undefined && cores >= from ? fast : slow;
	return { generation, tier, cores, bandwidth: gbps * 1e9, flops: cores * perCore * 1e12 };
};

/** A model with fewer active than total parameters, a mixture of experts. */
const isMoe = (model: Pick<CatalogModel, "params_b" | "active_params_b">): boolean =>
	model.active_params_b !== null &&
	model.params_b !== null &&
	model.active_params_b < model.params_b * 0.5;

/** Bytes a Q4 file holds per parameter, for a model whose card gives no count. */
const BYTES_PER_PARAM_Q4 = 0.6;

/** The parameters each token runs through, in units. */
const activeParams = (
	model: Pick<CatalogModel, "params_b" | "active_params_b">,
	bytes: number
): number => (model.active_params_b ?? model.params_b ?? bytes / BYTES_PER_PARAM_Q4 / 1e9) * 1e9;

/**
 * How many tokens per second a model writes on a chip: the bandwidth over
 * the bytes each token reads, at 30-50% for a mixture of experts and 60-80%
 * for a dense model.
 */
export const decodeTokensPerSecond = (
	chip: Chip,
	model: Pick<CatalogModel, "params_b" | "active_params_b">,
	bytes: number
): Range => {
	const share =
		model.active_params_b !== null && model.params_b !== null && model.params_b > 0
			? Math.min(1, model.active_params_b / model.params_b)
			: 1;
	const ceiling = chip.bandwidth / (bytes * share);
	const [low, high] = isMoe(model) ? [0.3, 0.5] : [0.6, 0.8];
	return { low: ceiling * low, high: ceiling * high };
};

/**
 * How many prompt tokens per second a model reads on a chip: the GPU's
 * throughput over two operations per active parameter, at 35-65% for a
 * mixture of experts and 60-90% for a dense model.
 */
export const prefillTokensPerSecond = (
	chip: Chip,
	model: Pick<CatalogModel, "params_b" | "active_params_b">,
	bytes: number
): Range => {
	const ceiling = chip.flops / (2 * activeParams(model, bytes));
	const [low, high] = isMoe(model) ? [0.35, 0.65] : [0.6, 0.9];
	return { low: ceiling * low, high: ceiling * high };
};

/** The seconds a cold first turn takes to read Claude Code's prompt. */
export const coldFirstTurnSeconds = (prefill: Range): Range => ({
	low: CLAUDE_CODE_PROMPT_TOKENS / prefill.high,
	high: CLAUDE_CODE_PROMPT_TOKENS / prefill.low,
});

/** What a model file would be like on a device. */
export type Estimate = {
	/** Whether it fits. */
	readonly fit: Fit;
	/** The memory it needs in bytes. */
	readonly need: number;
	/** The GPU budget in bytes. */
	readonly budget: number;
	/** Tokens it writes per second, when the chip is known. */
	readonly decode: Range | null;
	/** Prompt tokens it reads per second, when the chip is known. */
	readonly prefill: Range | null;
	/** Seconds a cold Claude Code first turn takes, when the chip is known. */
	readonly coldTurn: Range | null;
};

/** The settings a load takes its context and KV type from. */
export type LoadSettings = { readonly context: number; readonly kvType: KvCacheType };

/**
 * What a catalog file would be like on a device with these settings, or
 * null for a device without a GPU, which is too slow for agents.
 */
export const estimate = (
	hardware: DeviceHardware,
	model: CatalogModel,
	file: Pick<CatalogFile, "bytes">,
	settings: LoadSettings,
	slots = 1
): Estimate | null => {
	const budget = gpuBudget(hardware);
	if (budget === null) return null;
	const context = Math.min(settings.context, model.native_context);
	const need = memoryNeeded({
		bytes: file.bytes,
		kv: model.kv,
		context,
		kvType: settings.kvType,
		slots,
	});
	const chip = chipOf(hardware);
	const prefill = chip && prefillTokensPerSecond(chip, model, file.bytes);
	return {
		fit: fitOf(need, budget),
		need,
		budget,
		decode: chip && decodeTokensPerSecond(chip, model, file.bytes),
		prefill,
		coldTurn: prefill && coldFirstTurnSeconds(prefill),
	};
};

/** The file a catalog model recommends, else its first. */
export const recommendedFile = (model: CatalogModel): CatalogFile | undefined =>
	model.files.find((f) => f.recommended === true) ?? model.files[0];
