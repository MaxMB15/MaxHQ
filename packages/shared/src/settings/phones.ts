// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the Phones settings work from: each phone you paired with the
// devices that have it and whether each lets it do everything, which
// devices Add phone ticks, and the one it pairs through.

import type { DeviceId, PhoneDevice, PhoneId, Timestamp } from "../bindings";
import { alwaysOn } from "../tasks/place";
import { all, type ViewState } from "../view/replica";
import { deviceName } from "../view/select";

/** A phone, as every device that has it tells. */
export type PairedPhone = {
	readonly id: PhoneId;
	readonly name: string;
	/** When it last paired. */
	readonly pairedAt: Timestamp;
	/** The devices that let it in, by name, with whether each lets it do everything. */
	readonly devices: readonly {
		readonly device: DeviceId;
		readonly name: string;
		readonly everything: boolean;
	}[];
	/** Whether any device lets it do everything. */
	readonly everything: boolean;
};

/** Each phone paired with a device, by name. */
export const pairedPhones = (view: ViewState): PairedPhone[] => {
	const phones = new Map<PhoneId, PairedPhone & { devices: PairedPhone["devices"][number][] }>();
	for (const phone of all(view, "phone")) {
		const seen = phones.get(phone.id);
		const device = {
			device: phone.device,
			name: deviceName(view, phone.device),
			everything: phone.everything ?? false,
		};
		if (!seen) {
			phones.set(phone.id, {
				id: phone.id,
				name: phone.name,
				pairedAt: phone.paired_at,
				devices: [device],
				everything: false,
			});
			continue;
		}
		seen.devices.push(device);
		if (phone.paired_at > seen.pairedAt) {
			phones.set(phone.id, { ...seen, name: phone.name, pairedAt: phone.paired_at });
		}
	}
	return [...phones.values()]
		.map((phone) => ({
			...phone,
			devices: phone.devices.sort((a, b) => a.name.localeCompare(b.name)),
			everything: phone.devices.some((d) => d.everything),
		}))
		.sort((a, b) => a.name.localeCompare(b.name) || a.id.localeCompare(b.id));
};

/** The devices Add phone ticks first: each one a phone can reach. */
export const reachable = (devices: readonly PhoneDevice[]): DeviceId[] =>
	devices.filter((d) => d.note === null).map((d) => d.device);

/**
 * The device to pair through, of those ticked: the one that stays on,
 * then any that isn't a Mac, since a laptop sleeps, then the first.
 */
export const pairingDevice = (view: ViewState, ticked: readonly DeviceId[]): DeviceId | null => {
	const on = alwaysOn(view);
	if (on !== null && ticked.includes(on)) return on;
	return ticked.find((d) => view.tables.device.get(d)?.os !== "macos") ?? ticked[0] ?? null;
};
