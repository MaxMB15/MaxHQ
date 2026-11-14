// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: choosing the
// least loaded device, with the same score and the same skips, rewritten as
// an arrow function over `HostLoad`'s field names. The names of the
// placements are MaxHQ's.

import type { DeviceId, Timestamp } from "../bindings";
import type { HostLoad, Placement } from "./wire";

/** One device the balancer may pick. */
export type Candidate = {
	readonly device: DeviceId;
	readonly load: HostLoad | null;
	/** When this client got the sample, which avoids comparing two machines' clocks. */
	readonly receivedAt?: Timestamp;
	readonly weight: number;
};

/** What each placement weighs. */
export const PLACEMENT_WEIGHT: Readonly<Record<Placement, number>> = {
	prefer: 100,
	normal: 50,
};

/**
 * The device with the most free CPU and memory for its weight, or null when
 * none has a fresh sample with room. Pass only connected devices that can
 * run the agent.
 */
export const chooseDevice = (candidates: readonly Candidate[], now: Timestamp): DeviceId | null => {
	let selected: DeviceId | null = null;
	let bestScore = 0;
	for (const { device, load, receivedAt, weight } of candidates) {
		const sampledAt = receivedAt ?? load?.sampled_at ?? 0;
		if (
			!load ||
			!Number.isFinite(weight) ||
			weight <= 0 ||
			now - sampledAt > 15_000 ||
			sampledAt > now + 5_000 ||
			load.cpu_util === null ||
			load.cpu_util >= 0.95 ||
			load.mem_total <= 0 ||
			load.cpu_count <= 0
		) {
			continue;
		}
		const memoryAvailable = load.mem_available / load.mem_total;
		if (memoryAvailable <= 0.05) continue;
		const score = weight * load.cpu_count * (1 - load.cpu_util) * memoryAvailable;
		if (score > bestScore) {
			selected = device;
			bestScore = score;
		}
	}
	return selected;
};
