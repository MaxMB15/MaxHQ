// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The device links of the sample data: the Mac and devbox link to each
// other until you turn the pair off. They live in memory.

import type { DeviceId, PeerLink } from "../bindings";
import { DEVBOX, MAC } from "./mock-view";
import type { DataSource } from "./source";

const NAMES: Record<DeviceId, string> = { [MAC]: "MacBook Pro", [DEVBOX]: "devbox" };

/** The same pair either way round. */
const pair = (a: DeviceId, b: DeviceId) => [a, b].sort().join(" ");

/** The device link calls over the sample devices. */
export const mockLinks = (): Pick<DataSource, "listDeviceLinks" | "setDeviceLink"> => {
	const off = new Set<string>();
	return {
		listDeviceLinks: async (device) => ({
			status: "ok",
			data: Object.keys(NAMES)
				.filter((peer) => device in NAMES && peer !== device)
				.map((peer): PeerLink => ({
					peer,
					name: NAMES[peer] ?? peer,
					state: off.has(pair(device, peer)) ? "off" : "linked",
					note: null,
				})),
		}),
		setDeviceLink: async (device, peer, on) => {
			if (on) off.delete(pair(device, peer));
			else off.add(pair(device, peer));
			return { status: "ok", data: null };
		},
	};
};
