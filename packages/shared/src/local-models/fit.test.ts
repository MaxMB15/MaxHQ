// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { CatalogModel, DeviceHardware } from "../bindings";
import { mockCatalog } from "../data/mock-local-models";

import {
	chipOf,
	CLAUDE_CODE_PROMPT_TOKENS,
	estimate,
	estimatedGpuBudget,
	fitOf,
	GIB,
	kvBytes,
	memoryNeeded,
	OVERHEAD_BYTES,
	recommendedFile,
} from "./fit";
import { coldTurnText, fitLabel, formatMemory, speedText } from "./wording";

/** An M4 Max with 36 GB, its GPU allowed 27 GB as Metal reports it. */
const M4_MAX_36: DeviceHardware = {
	chip: "Apple M4 Max",
	gpu_cores: 32,
	ram: 36 * GIB,
	gpu: { budget: 27 * GIB, source: "metal" },
};

const catalog = mockCatalog();
const model = (id: string): CatalogModel => catalog.find((m) => m.id === id)!;
const settings = { context: 65_536, kvType: "f16" } as const;

describe("fit on an M4 Max with 36 GB", () => {
	it("fits the default model, and two slots of it are tight", () => {
		const qwen = model("qwen3.6-35b-a3b");
		const file = recommendedFile(qwen)!;
		expect(file.name).toMatch(/UD-Q4_K_XL/);
		const one = estimate(M4_MAX_36, qwen, file, settings)!;
		expect(one.fit).toBe("fits");
		expect(one.budget).toBe(27 * GIB);
		const two = estimate(M4_MAX_36, qwen, file, settings, 2)!;
		expect(two.fit).toBe("tight");
		expect(fitLabel(one.fit)).toBe("Fits");
		expect(fitLabel(two.fit)).toBe("Tight");
	});

	it("calls a 46 GB model too large", () => {
		const coder = model("qwen3-coder-next-80b-a3b");
		const file = recommendedFile(coder)!;
		expect(file.bytes).toBe(46_000_000_000);
		const result = estimate(M4_MAX_36, coder, file, settings)!;
		expect(result.fit).toBe("too_large");
		expect(fitLabel(result.fit)).toBe("Too large");
	});

	it("estimates speed from the chip's bandwidth and a cold Claude Code turn", () => {
		const qwen = model("qwen3.6-35b-a3b");
		const result = estimate(M4_MAX_36, qwen, recommendedFile(qwen)!, settings)!;
		// 410 GB/s over the 3B active of 35B parameters' share of 22.4 GB, at 30-50%.
		const ceiling = 410e9 / (22.4e9 * (3 / 35));
		expect(result.decode!.low).toBeCloseTo(ceiling * 0.3, 3);
		expect(result.decode!.high).toBeCloseTo(ceiling * 0.5, 3);
		expect(result.coldTurn!.low).toBeCloseTo(
			CLAUDE_CODE_PROMPT_TOKENS / result.prefill!.high,
			6
		);
		expect(result.coldTurn!.high).toBeCloseTo(
			CLAUDE_CODE_PROMPT_TOKENS / result.prefill!.low,
			6
		);
		expect(speedText(result.decode!)).toMatch(/^\d+-\d+ tokens\/s$/);
		expect(coldTurnText(result.coldTurn!)).toMatch(/^First turn about \d+-\d+ (s|min)$/);
	});

	it("caps the context at the model's own", () => {
		const gpt = model("gpt-oss-20b");
		const file = recommendedFile(gpt)!;
		const big = estimate(M4_MAX_36, gpt, file, { context: 1_000_000, kvType: "f16" })!;
		const native = estimate(M4_MAX_36, gpt, file, {
			context: gpt.native_context,
			kvType: "f16",
		})!;
		expect(big.need).toBe(native.need);
	});
});

describe("the parts of a fit", () => {
	it("counts the KV cache per layer, with sliding-window layers held to their window", () => {
		const full = { full_layers: 10, kv_heads: 2, head_dim: 256 };
		expect(kvBytes(full, 65_536, "f16")).toBe(2 * 2 * 256 * 2 * 10 * 65_536);
		expect(kvBytes(full, 65_536, "q8_0")).toBe(((2 * 2 * 256 * 34) / 32) * 10 * 65_536);
		const sliding = {
			full_layers: 12,
			kv_heads: 8,
			head_dim: 64,
			swa_layers: 12,
			swa_window: 128,
		};
		expect(kvBytes(sliding, 65_536, "f16")).toBe(2 * 8 * 64 * 2 * (12 * 65_536 + 12 * 128));
		expect(kvBytes(sliding, 100, "f16")).toBe(2 * 8 * 64 * 2 * (12 * 100 + 12 * 100));
	});

	it("adds weights, KV per slot and about a gigabyte of buffers", () => {
		const kv = { full_layers: 1, kv_heads: 1, head_dim: 1 };
		expect(memoryNeeded({ bytes: 1000, kv, context: 10, kvType: "f16", slots: 3 })).toBe(
			1000 + 3 * 2 * 2 * 10 + OVERHEAD_BYTES
		);
	});

	it("leaves a tenth of the budget spare to call it a fit", () => {
		expect(fitOf(90, 100)).toBe("fits");
		expect(fitOf(91, 100)).toBe("tight");
		expect(fitOf(100, 100)).toBe("tight");
		expect(fitOf(101, 100)).toBe("too_large");
	});

	it("knows Apple chips by generation, tier and GPU cores", () => {
		expect(chipOf({ chip: "Apple M4 Max", gpu_cores: 32 })).toMatchObject({
			generation: 4,
			tier: "max",
			bandwidth: 410e9,
		});
		expect(chipOf({ chip: "Apple M4 Max", gpu_cores: 40 })?.bandwidth).toBe(546e9);
		expect(chipOf({ chip: "Apple M2 Ultra", gpu_cores: null })).toMatchObject({
			cores: 60,
			bandwidth: 800e9,
		});
		expect(chipOf({ chip: "Apple M3", gpu_cores: 10 })?.bandwidth).toBe(100e9);
		expect(chipOf({ chip: "Intel Xeon", gpu_cores: null })).toBeNull();
		expect(chipOf({ chip: null, gpu_cores: null })).toBeNull();
	});

	it("guesses macOS's GPU share when maxd couldn't read it", () => {
		expect(estimatedGpuBudget(16 * GIB) / GIB).toBeCloseTo(16 * (2 / 3), 6);
		expect(estimatedGpuBudget(36 * GIB)).toBe(27 * GIB);
		expect(formatMemory(27 * GIB)).toBe("27 GB");
	});

	it("gives no estimate without a GPU", () => {
		const qwen = model("qwen3.6-35b-a3b");
		const box: DeviceHardware = { chip: null, gpu_cores: null, ram: 64 * GIB, gpu: null };
		expect(estimate(box, qwen, recommendedFile(qwen)!, settings)).toBeNull();
	});

	it("still says whether it fits on a chip this build doesn't know", () => {
		const qwen = model("qwen3.6-35b-a3b");
		const other: DeviceHardware = { ...M4_MAX_36, chip: "Apple M9 Hyper" };
		const result = estimate(other, qwen, recommendedFile(qwen)!, settings)!;
		expect(result.fit).toBe("fits");
		expect(result.decode).toBeNull();
		expect(result.coldTurn).toBeNull();
	});
});
