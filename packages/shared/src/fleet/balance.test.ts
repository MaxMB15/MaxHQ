// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the load balancing
// cases, over `HostLoad`'s field names. The placement weights case is
// MaxHQ's.

import { describe, expect, it } from "vitest";

import { chooseDevice, PLACEMENT_WEIGHT } from "./balance";

describe("load balancing shared project machines", () => {
	const now = 100_000;
	const load = {
		sampled_at: now,
		cpu_util: 0.2,
		cpu_count: 8,
		mem_available: 8_000,
		mem_total: 16_000,
	};

	it("compares three machines using free capacity and preference", () => {
		const candidates = [
			{ device: "busy", load: { ...load, cpu_util: 0.9 }, weight: 1 },
			{ device: "idle", load, weight: 1 },
			{ device: "preferred", load: { ...load, cpu_count: 4 }, weight: 3 },
		];
		expect(chooseDevice(candidates, now)).toBe("preferred");
		expect(chooseDevice(candidates.slice(0, 2), now)).toBe("idle");
	});

	it("rejects stale, unknown, excluded and saturated machines", () => {
		expect(
			chooseDevice(
				[
					{ device: "stale", load: { ...load, sampled_at: now - 15_001 }, weight: 1 },
					{ device: "unknown", load: null, weight: 1 },
					{ device: "no-cpu-sample", load: { ...load, cpu_util: null }, weight: 1 },
					{ device: "excluded", load, weight: 0 },
					{ device: "cpu-full", load: { ...load, cpu_util: 0.95 }, weight: 1 },
					{ device: "memory-full", load: { ...load, mem_available: 100 }, weight: 1 },
				],
				now
			)
		).toBeNull();
	});

	it("uses client receipt time when host clocks differ", () => {
		const candidate = {
			device: "different-clock",
			load: { ...load, sampled_at: now + 60_000 },
			receivedAt: now,
			weight: 1,
		};
		expect(chooseDevice([candidate], now)).toBe("different-clock");
		expect(chooseDevice([candidate], now + 15_001)).toBeNull();
	});

	it("lets a preferred device win with less room, but not with none", () => {
		const roomy = { device: "roomy", load, weight: PLACEMENT_WEIGHT.normal };
		const tight = {
			device: "tight",
			load: { ...load, cpu_util: 0.5 },
			weight: PLACEMENT_WEIGHT.prefer,
		};
		expect(chooseDevice([roomy, tight], now)).toBe("tight");
		const full = { ...tight, load: { ...load, cpu_util: 0.97 } };
		expect(chooseDevice([roomy, full], now)).toBe("roomy");
	});
});
