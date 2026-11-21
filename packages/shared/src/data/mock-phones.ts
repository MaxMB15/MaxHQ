// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Phone pairing in the sample data: devbox takes phones, and the Mac does
// once its network access is on. One phone is paired with devbox, with its
// switch off. A pairing shows a code that's only its corner squares, and
// the addresses for the phone live in memory.

import type { DeviceId, NetworkAccessSettings, PhoneDevice } from "../bindings";
import type { Entity } from "../fleet/wire";
import { DEFAULT_PORT } from "../settings/network-access";
import { DEVBOX, MAC } from "./mock-view";
import type { DataSource } from "./source";

/** The private address each sample device listens on. */
const LAN: Record<DeviceId, string> = { [MAC]: "192.168.1.20", [DEVBOX]: "192.168.1.30" };

/** `device`'s network access as its maxd publishes it for `network`. */
export const mockNetworkAccess = (
	device: DeviceId,
	network: NetworkAccessSettings = {}
): Entity => {
	const on = network.on ?? false;
	return {
		entity: "network_access",
		device,
		on,
		port: network.port ?? DEFAULT_PORT,
		hosts: on ? [LAN[device] ?? "192.168.1.40"] : [],
		pin: on ? "mJ3x0q7wTn2cV9bLr4sYk8pZf1hGd6aEu5oWi0XtQeA" : null,
		problem: null,
	};
};

/** The sample phone, paired with devbox three days before `now`. */
export const mockPhone = (now: number): Entity => ({
	entity: "phone",
	id: "phone-sample",
	device: DEVBOX,
	name: "iPhone",
	ssh_public: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGV4YW1wbGUtcGhvbmUta2V5LWZvci10ZXN0cw",
	push_public: "cGhvbmUtcHVzaC1rZXktZm9yLXRoZS1zYW1wbGUtZGF0YQ",
	paired_at: now - 3 * 24 * 60 * 60 * 1000,
	push: [],
	everything: false,
});

/** The three corner squares of a QR code 21 modules on a side. */
const CORNERS = (
	[
		[0, 0],
		[14, 0],
		[0, 14],
	] as const
)
	.map(([x, y]) => `M${x} ${y}h7v7h-7zM${x + 1} ${y + 1}v5h5v-5zM${x + 2} ${y + 2}h3v3h-3z`)
	.join("");

/** The phone pairing calls over the sample devices. */
export const mockPhones = (
	now: () => number,
	networkOn: (device: DeviceId) => boolean
): Pick<DataSource, "listPhoneDevices" | "beginPhonePair" | "setPhoneAddress"> => {
	const addresses = new Map<DeviceId, string>();
	return {
		listPhoneDevices: async () => ({
			status: "ok",
			data: [
				{
					device: DEVBOX,
					name: "devbox",
					address: addresses.get(DEVBOX) ?? null,
					note: null,
				},
				{
					device: MAC,
					name: "MacBook Pro",
					address: addresses.get(MAC) ?? null,
					note: networkOn(MAC)
						? null
						: "Turn on Network access on its tab to pair a phone with it.",
				},
			] satisfies PhoneDevice[],
		}),
		beginPhonePair: async (through) => ({
			status: "ok",
			data: {
				pair: "pair-1",
				device: through,
				qr: { size: 21, path: CORNERS },
				expires_at: now() + 10 * 60 * 1000,
			},
		}),
		setPhoneAddress: async (device, address) => {
			if (address === null) addresses.delete(device);
			else addresses.set(device, address);
			return { status: "ok", data: null };
		},
	};
};
