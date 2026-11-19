// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a device's settings page works out without React Native: what it
// shows while a save is on its way, what a number field holds while you
// type, and which cards the device gets besides the ones every device has.

import type { DeviceId } from "@maxhq/shared/bindings";
import { isThisMac, listsAppKeys } from "@maxhq/shared/settings/app-keys";
import { type DeviceFields, sameDeviceFields } from "@maxhq/shared/settings/device-settings";
import { offersSkills } from "@maxhq/shared/skills/library";
import type { ViewState } from "@maxhq/shared/view/replica";

/** What you just picked, and the settings the device had then. */
export type Pending = { readonly fields: DeviceFields; readonly from: DeviceFields };

/** What you just picked, until the view moves on from what it had then. */
export const shownFields = (pending: Pending | null, current: DeviceFields): DeviceFields =>
	pending && sameDeviceFields(pending.from, current) ? pending.fields : current;

/** What you typed in a number field, and the value it held when you started. */
export type Typed = { readonly text: string; readonly from: number };

/**
 * What you typed and haven't saved, or null. Typing from before a save
 * elsewhere is dropped, not saved over it.
 */
export const draftOf = (typed: Typed | null, value: number): string | null =>
	typed && typed.from === value ? typed.text : null;

/** The cards only some devices get. */
export type SomeCards = {
	/** Sleep and Cursor usage. */
	readonly mac: boolean;
	/** New workspaces, on a device with Incus. */
	readonly workspaces: boolean;
	readonly library: boolean;
	readonly network: boolean;
	readonly appKeys: boolean;
};

export const someCards = (view: ViewState, device: DeviceId): SomeCards => {
	const record = view.tables.device.get(device);
	return {
		mac: record?.os === "macos",
		workspaces: Boolean(record?.workspace_host),
		library: offersSkills(view, device),
		network: view.tables.network_access.has(device),
		appKeys: isThisMac(view, device) || listsAppKeys(view, device),
	};
};
