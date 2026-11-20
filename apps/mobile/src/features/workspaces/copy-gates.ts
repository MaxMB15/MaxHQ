// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Which devices must let this phone do everything before it can copy a
// project. The device the copy goes to always does: it makes a workspace,
// a container or a clone in a folder. The device it comes from does once
// any file is ticked, since its maxd hands those files over.

import type { DeviceId } from "@maxhq/shared/bindings";
import { formatList } from "@maxhq/shared/i18n";
import type { ViewState } from "@maxhq/shared/view/replica";
import { deviceName } from "@maxhq/shared/view/select";

/** The devices a copy needs the switch on, or null where it needs none. */
export type CopyGates = { readonly target: DeviceId | null; readonly source: DeviceId | null };

/** Whether each of those devices lets this phone. */
export type CopyLets = { readonly target: boolean; readonly source: boolean };

/** The devices the copy needs the switch on, given where it goes and how many files it brings. */
export const copyGates = (
	target: DeviceId | null,
	source: DeviceId | null,
	files: number
): CopyGates => ({ target, source: files > 0 ? source : null });

/** Whether every device the copy needs lets this phone. */
export const gatesOpen = (gates: CopyGates, lets: CopyLets): boolean =>
	(gates.target === null || lets.target) && (gates.source === null || lets.source);

/** The names of the devices the copy needs that don't let this phone, each once. */
export const closedGates = (view: ViewState, gates: CopyGates, lets: CopyLets): string[] => {
	const closed = [
		gates.target !== null && !lets.target ? gates.target : null,
		gates.source !== null && !lets.source ? gates.source : null,
	].filter((device): device is DeviceId => device !== null);
	return [...new Set(closed)].map((device) => deviceName(view, device));
};

/** The closed devices in words, such as "devbox and studio". */
export const closedText = (names: readonly string[]): string => formatList(names);
