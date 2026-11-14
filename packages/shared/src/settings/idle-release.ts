// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How long an agent sits idle before its device's `maxd` stops its CLI to
// free the memory. The agent stays as it is, and its CLI starts again for
// the next message. A `maxd` from before this setting doesn't report it.

import { t } from "../i18n";

import type { DeviceId } from "../bindings";
import type { Option } from "../new-agent/spec";
import type { ViewState } from "../view/replica";

/** The idle minutes before a CLI stops, unless a device says otherwise. */
export const DEFAULT_IDLE_RELEASE_MINUTES = 60;

/** The times Settings offers, in minutes, with 0 for never. */
export const IDLE_RELEASE_MINUTES = [0, 15, 30, 60, 120, 240, 480] as const;

/** Whether a device's `maxd` stops idle CLIs, as its settings show. */
export const releasesIdle = (view: ViewState, device: DeviceId): boolean =>
	view.tables.device_settings.get(device)?.idle_release_minutes !== undefined;

/** The minutes in words, such as "15 minutes" or "2 hours", or "Never" for 0. */
export const idleReleaseText = (minutes: number): string => {
	if (minutes === 0) return t("Never");
	const hours = minutes / 60;
	if (Number.isInteger(hours)) return `${hours} ${hours === 1 ? "hour" : "hours"}`;
	return `${minutes} ${minutes === 1 ? "minute" : "minutes"}`;
};

/**
 * The times to pick from. One set elsewhere that isn't among them shows
 * too, so the pick never reads as nothing.
 */
export const idleReleaseOptions = (current: number): Option[] => {
	const minutes: number[] = [...IDLE_RELEASE_MINUTES];
	if (!minutes.includes(current)) minutes.push(current);
	return minutes
		.sort((a, b) => a - b)
		.map((value) => ({ value: String(value), label: idleReleaseText(value) }));
};
