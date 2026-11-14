// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How much of each agent's timeline a device's `maxd` keeps: its newest
// items in memory, the rest on disk, and past a size the oldest leave the
// timeline. A `maxd` from before these settings doesn't report them.

import { t } from "../i18n";

import type { DeviceId } from "../bindings";
import type { Option } from "../new-agent/spec";
import type { ViewState } from "../view/replica";

/** The items each agent keeps in memory, unless a device says otherwise. */
export const DEFAULT_MEMORY_ITEMS = 256;

/** The counts Settings offers, which span all `maxd` takes. */
export const MEMORY_ITEMS = [64, 128, 256, 512, 1024, 2048, 4096] as const;

/** The MiB of history each agent keeps, unless a device says otherwise. */
export const DEFAULT_HISTORY_MIB = 256;

/** The sizes Settings offers, in MiB, with 0 for no limit. */
export const HISTORY_MIB = [32, 64, 128, 256, 512, 1024, 2048, 4096, 0] as const;

/** What no limit risks. */
export const NO_LIMIT_WARNING =
	"An agent's timeline can grow without end on disk, and a restart reads its whole history, which can take minutes for a long one.";

/** Whether a device's `maxd` takes these settings, as its settings show. */
export const keepsMemoryLimits = (view: ViewState, device: DeviceId): boolean =>
	view.tables.device_settings.get(device)?.memory_items !== undefined;

/** The count in words, such as "256 items". */
export const memoryItemsText = (items: number): string => t("{{value1}} items", { value1: items });

/** The size in words, such as "256 MB" or "2 GB", or "No limit" for 0. */
export const historyText = (mib: number): string => {
	if (mib === 0) return t("No limit");
	const gib = mib / 1024;
	return Number.isInteger(gib)
		? t("{{value1}} GB", { value1: gib })
		: t("{{value1}} MB", { value1: mib });
};

// The values offered, and one set elsewhere that isn't among them, so the
// pick never reads as nothing.
const picks = (offered: readonly number[], current: number): number[] =>
	offered.includes(current) ? [...offered] : [...offered, current];

/** The counts to pick from, fewest first. */
export const memoryItemsOptions = (current: number): Option[] =>
	picks(MEMORY_ITEMS, current)
		.sort((a, b) => a - b)
		.map((items) => ({ value: String(items), label: memoryItemsText(items) }));

/** The sizes to pick from, smallest first, then No limit with what it risks. */
export const historyOptions = (current: number): Option[] =>
	picks(HISTORY_MIB, current)
		.sort((a, b) => (a || Infinity) - (b || Infinity))
		.map((mib) => ({
			value: String(mib),
			label: historyText(mib),
			...(mib === 0 ? { detail: NO_LIMIT_WARNING, warn: true } : {}),
		}));
